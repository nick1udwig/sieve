#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer, SummaryOutcome};
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::{BwrapQuarantineRunner, QuarantineRunError, QuarantineRunner};
use sieve_runtime::{
    Clock, EventLogError, InProcessApprovalBus, RuntimeDeps, RuntimeDisposition, RuntimeEventLog,
    RuntimeOrchestrator, ShellRunRequest,
};
use sieve_shell::{BasicShellAnalyzer, ShellAnalysis, ShellAnalysisError, ShellAnalyzer};
use sieve_types::{
    ApprovalAction, ApprovalRequestedEvent, ApprovalResolvedEvent, CapacityType, CommandKnowledge,
    CommandSegment, EndorseRequest, Integrity, LlmModelConfig, LlmProvider, PlannerTurnInput,
    PlannerTurnOutput, QuarantineReport, QuarantineRunRequest, RunId, RuntimeEvent, SinkKey,
    Source, UncertainMode, UnknownMode, ValueLabel, ValueRef,
};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{sleep, Duration};

#[derive(Default)]
struct VecEventLog {
    events: StdMutex<Vec<RuntimeEvent>>,
}

#[async_trait]
impl RuntimeEventLog for VecEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        self.events
            .lock()
            .map_err(|_| EventLogError::Append("event log lock poisoned".to_string()))?
            .push(event);
        Ok(())
    }
}

impl VecEventLog {
    fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("event lock").clone()
    }
}

struct TestClock {
    now: AtomicU64,
}

impl TestClock {
    fn new(start: u64) -> Self {
        Self {
            now: AtomicU64::new(start),
        }
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::Relaxed)
    }
}

struct NoopPlanner {
    config: LlmModelConfig,
}

#[async_trait]
impl PlannerModel for NoopPlanner {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, _input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        })
    }
}

struct StaticShell {
    analysis: ShellAnalysis,
}

impl ShellAnalyzer for StaticShell {
    fn analyze_shell_lc_script(&self, _script: &str) -> Result<ShellAnalysis, ShellAnalysisError> {
        Ok(self.analysis.clone())
    }
}

struct StaticSummaries {
    outcome: SummaryOutcome,
}

impl CommandSummarizer for StaticSummaries {
    fn summarize(&self, _argv: &[String]) -> SummaryOutcome {
        self.outcome.clone()
    }
}

#[derive(Default)]
struct RecordingQuarantine {
    calls: StdMutex<Vec<QuarantineRunRequest>>,
}

#[async_trait]
impl QuarantineRunner for RecordingQuarantine {
    async fn run(
        &self,
        request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError> {
        self.calls
            .lock()
            .map_err(|_| QuarantineRunError::Exec("quarantine lock poisoned".to_string()))?
            .push(request.clone());
        Ok(QuarantineReport {
            run_id: request.run_id.clone(),
            trace_path: format!("/tmp/sieve-e2e/{}", request.run_id.0),
            stdout_path: None,
            stderr_path: None,
            attempted_capabilities: Vec::new(),
            exit_code: Some(0),
        })
    }
}

impl RecordingQuarantine {
    fn calls(&self) -> Vec<QuarantineRunRequest> {
        self.calls.lock().expect("calls lock").clone()
    }
}

fn mk_runtime(
    shell: Arc<dyn ShellAnalyzer>,
    summaries: Arc<dyn CommandSummarizer>,
    policy_toml: &str,
    quarantine: Arc<dyn QuarantineRunner>,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<InProcessApprovalBus>,
    Arc<VecEventLog>,
) {
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let policy = Arc::new(TomlPolicyEngine::from_toml_str(policy_toml).expect("policy parse"));
    let planner = Arc::new(NoopPlanner {
        config: LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model: "gpt-test".to_string(),
            api_base: None,
        },
    });
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell,
        summaries,
        policy,
        quarantine,
        planner,
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(TestClock::new(1000)),
    }));
    (runtime, approval_bus, event_log)
}

fn label_with_sinks(integrity: Integrity, sinks: &[&str]) -> ValueLabel {
    let mut provenance = BTreeSet::new();
    provenance.insert(Source::User);
    let allowed_sinks = sinks
        .iter()
        .map(|sink| SinkKey((*sink).to_string()))
        .collect();
    ValueLabel {
        integrity,
        provenance,
        allowed_sinks,
        capacity_type: CapacityType::Enum,
    }
}

async fn wait_for_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
    for _ in 0..30 {
        let published = bus.published_events().expect("published events");
        if let Some(first) = published.first() {
            return first.clone();
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("approval not requested in time");
}

async fn wait_for_approval_count(
    bus: &InProcessApprovalBus,
    count: usize,
) -> Vec<ApprovalRequestedEvent> {
    for _ in 0..30 {
        let published = bus.published_events().expect("published events");
        if published.len() >= count {
            return published;
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("approval count not reached in time");
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}

fn write_fake_bwrap(path: &Path) {
    fs::write(
        path,
        "#!/usr/bin/env bash\nset -euo pipefail\ntrace_base=\"\"\nfor ((i=1;i<=${#};i++)); do\n  arg=\"${!i}\"\n  if [[ \"$arg\" == \"-o\" ]]; then\n    next=$((i+1))\n    trace_base=\"${!next}\"\n  fi\ndone\necho 'execve(\"/bin/echo\", [\"echo\"], 0x0) = 0' > \"${trace_base}.123\"\necho fake-stdout\necho fake-stderr >&2\n",
    )
    .expect("write fake bwrap");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod fake bwrap");
}

#[tokio::test]
async fn rm_rf_is_gated_by_deny_with_approval() {
    let policy_toml = r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"
reason = "rm -rf requires approval"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(RecordingQuarantine::default());
    let (runtime, approval_bus, event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        quarantine,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-rm".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "rm -rf /tmp/demo".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "deny-rm-rf");
    assert_eq!(
        requested.command_segments[0].argv,
        vec!["rm".to_string(), "-rf".to_string(), "/tmp/demo".to_string()]
    );
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1100,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(disposition, RuntimeDisposition::ExecuteMainline);

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[2], RuntimeEvent::ApprovalResolved(_)));
}

#[tokio::test]
async fn curl_post_payload_sink_enforced_then_declassify_allows_flow() {
    let policy_toml = r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        Arc::new(RecordingQuarantine::default()),
    );
    runtime
        .upsert_value_label(
            ValueRef("argv:5".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed payload value");

    let first = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-curl-1".to_string()),
            cwd: "/tmp".to_string(),
            script: "curl -X POST https://api.example.com/v1/upload -d body".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    match first {
        RuntimeDisposition::Denied { reason } => {
            assert!(reason.contains("value argv:5 cannot flow to sink"));
            assert!(reason.contains("https://api.example.com/v1/upload"));
        }
        other => panic!("expected sink denial, got {other:?}"),
    }

    let declassify_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-declassify-1".to_string()),
                    sieve_types::DeclassifyRequest {
                        value_ref: ValueRef("argv:5".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };
    let first_requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: first_requested.request_id.clone(),
            run_id: first_requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1200,
        })
        .expect("resolve declassify approval");
    let first_transition = declassify_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert!(!first_transition.sink_was_already_allowed);

    let second = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-curl-2".to_string()),
            cwd: "/tmp".to_string(),
            script: "curl -X POST https://api.example.com/v1/upload -d body".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert_eq!(second, RuntimeDisposition::ExecuteMainline);

    let second_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-declassify-2".to_string()),
                    sieve_types::DeclassifyRequest {
                        value_ref: ValueRef("argv:5".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };
    let second_requested = wait_for_approval_count(&approval_bus, 2).await[1].clone();
    assert_ne!(first_requested.request_id, second_requested.request_id);
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: second_requested.request_id,
            run_id: second_requested.run_id,
            action: ApprovalAction::Deny,
            created_at_ms: 1201,
        })
        .expect("resolve second declassify approval");
    let second_transition = second_task
        .await
        .expect("task join")
        .expect("runtime ok");
    assert!(second_transition.is_none());
}

#[tokio::test]
async fn unknown_modes_cover_deny_accept_and_ask() {
    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(RecordingQuarantine::default());
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Unknown,
                segments: vec![CommandSegment {
                    argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
                    operator_before: None,
                }],
                unsupported_constructs: Vec::new(),
            },
        }),
        Arc::new(StaticSummaries {
            outcome: SummaryOutcome {
                knowledge: CommandKnowledge::Unknown,
                summary: None,
                reason: None,
            },
        }),
        policy_toml,
        quarantine.clone(),
    );

    let deny = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-unknown-deny".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert_eq!(
        deny,
        RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string(),
        }
    );
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert!(quarantine.calls().is_empty());

    let accept = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-unknown-accept".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Accept,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert!(matches!(accept, RuntimeDisposition::ExecuteQuarantine(_)));
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert_eq!(quarantine.calls().len(), 1);

    let ask_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-unknown-ask".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "custom-cmd --flag".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Ask,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };
    let requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    assert_eq!(requested.blocked_rule_id, "unknown_command_mode");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1300,
        })
        .expect("resolve unknown ask approval");
    let ask = ask_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(ask, RuntimeDisposition::ExecuteQuarantine(_)));
    assert_eq!(quarantine.calls().len(), 2);
}

#[tokio::test]
async fn uncertain_modes_cover_deny_accept_and_ask() {
    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(RecordingQuarantine::default());
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Uncertain,
                segments: vec![CommandSegment {
                    argv: vec!["weird-shell-construct".to_string()],
                    operator_before: None,
                }],
                unsupported_constructs: vec!["substitution_or_expansion".to_string()],
            },
        }),
        Arc::new(StaticSummaries {
            outcome: SummaryOutcome {
                knowledge: CommandKnowledge::Uncertain,
                summary: None,
                reason: Some("unsupported shell construct".to_string()),
            },
        }),
        policy_toml,
        quarantine.clone(),
    );

    let deny = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-uncertain-deny".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert_eq!(
        deny,
        RuntimeDisposition::Denied {
            reason: "uncertain command denied by mode".to_string(),
        }
    );
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert!(quarantine.calls().is_empty());

    let accept = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-uncertain-accept".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Accept,
        })
        .await
        .expect("runtime ok");
    assert!(matches!(accept, RuntimeDisposition::ExecuteQuarantine(_)));
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert_eq!(quarantine.calls().len(), 1);

    let ask_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-uncertain-ask".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "weird-shell-construct".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Ask,
                })
                .await
        })
    };
    let requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    assert_eq!(requested.blocked_rule_id, "uncertain_command_mode");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1301,
        })
        .expect("resolve uncertain ask approval");
    let ask = ask_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(ask, RuntimeDisposition::ExecuteQuarantine(_)));
    assert_eq!(quarantine.calls().len(), 2);
}

#[tokio::test]
async fn endorse_requires_fresh_approval_each_invocation() {
    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        Arc::new(RecordingQuarantine::default()),
    );
    runtime
        .upsert_value_label(
            ValueRef("v-control".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value label");

    let first_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .endorse_value_once(
                    RunId("run-endorse-1".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v-control".to_string()),
                        target_integrity: Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };
    let first_requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: first_requested.request_id.clone(),
            run_id: first_requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1400,
        })
        .expect("resolve first endorse");
    let first_transition = first_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert_eq!(first_transition.to_integrity, Integrity::Trusted);

    let second_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .endorse_value_once(
                    RunId("run-endorse-2".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v-control".to_string()),
                        target_integrity: Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };
    let second_requested = wait_for_approval_count(&approval_bus, 2).await[1].clone();
    assert_ne!(first_requested.request_id, second_requested.request_id);
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: second_requested.request_id.clone(),
            run_id: second_requested.run_id.clone(),
            action: ApprovalAction::Deny,
            created_at_ms: 1401,
        })
        .expect("resolve second endorse");
    let second_transition = second_task
        .await
        .expect("task join")
        .expect("runtime ok");
    assert!(second_transition.is_none());
}

#[tokio::test]
async fn unknown_accept_path_generates_quarantine_report_json() {
    let root = unique_temp_dir("sieve-runtime-e2e");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    let fake_bwrap = bin_dir.join("fake-bwrap");
    write_fake_bwrap(&fake_bwrap);

    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(BwrapQuarantineRunner::with_programs(
        root.join(".sieve/logs/traces"),
        fake_bwrap.to_string_lossy().to_string(),
        "strace",
        "bash",
    ));
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        quarantine,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-quarantine".to_string()),
            cwd: "/".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Accept,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());

    let report = match disposition {
        RuntimeDisposition::ExecuteQuarantine(report) => report,
        other => panic!("expected quarantine disposition, got {other:?}"),
    };
    let report_json_path = PathBuf::from(&report.trace_path).join("report.json");
    let report_json = fs::read_to_string(&report_json_path).expect("report json");
    assert!(report_json.contains("\"run_id\": \"run-quarantine\""));
    assert!(report_json.contains("\"trace_files\": ["));
    assert!(report_json.contains("strace.123"));
    assert!(report_json.contains("\"attempted_capabilities\": ["));

    fs::remove_dir_all(root).expect("cleanup");
}
