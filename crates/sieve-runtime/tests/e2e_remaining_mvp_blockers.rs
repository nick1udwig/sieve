#![forbid(unsafe_code)]

mod common;

use async_trait::async_trait;
use common::{
    wait_for_approval_count, RecordingQuarantine, StaticShell, StaticSummaries, VecEventLog,
};
use serde_json::json;
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer, SummaryOutcome};
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::QuarantineRunner;
use sieve_runtime::{
    BashMainlineRunner, InProcessApprovalBus, MainlineRunError, MainlineRunReport,
    MainlineRunRequest, MainlineRunner, PlannerRunRequest, RuntimeDeps, RuntimeDisposition,
    RuntimeError, RuntimeEventLog, RuntimeOrchestrator, ShellRunRequest, SystemClock,
    WebSearchError, WebSearchRunner,
};
use sieve_shell::{BasicShellAnalyzer, ShellAnalysis};
use sieve_types::{
    ApprovalAction, ApprovalResolvedEvent, BraveSearchRequest, BraveSearchResponse,
    CommandKnowledge, CommandSegment, CommandSummary, EndorseRequest, Integrity, LlmModelConfig,
    LlmProvider, PlannerToolCall, PlannerTurnInput, PlannerTurnOutput, RunId, RuntimeEvent,
    UncertainMode, UnknownMode, ValueRef,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::Arc;

const BASE_POLICY: &str = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;

struct StaticPlanner {
    output: PlannerTurnOutput,
}

impl StaticPlanner {
    fn new(output: PlannerTurnOutput) -> Self {
        Self { output }
    }
}

#[async_trait]
impl PlannerModel for StaticPlanner {
    fn config(&self) -> &LlmModelConfig {
        static CONFIG: std::sync::OnceLock<LlmModelConfig> = std::sync::OnceLock::new();
        CONFIG.get_or_init(|| LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model: "planner-test".to_string(),
            api_base: None,
        })
    }

    async fn plan_turn(&self, _input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        Ok(self.output.clone())
    }
}

struct NoopMainline;

#[async_trait]
impl MainlineRunner for NoopMainline {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: Some(0),
            artifacts: Vec::new(),
        })
    }
}

struct NoopWebSearch;

#[async_trait]
impl WebSearchRunner for NoopWebSearch {
    fn connect_scope(&self) -> String {
        "https://api.search.brave.com/res/v1/web/search".to_string()
    }

    async fn search(
        &self,
        request: BraveSearchRequest,
    ) -> Result<BraveSearchResponse, WebSearchError> {
        Ok(BraveSearchResponse {
            query: request.query,
            results: Vec::new(),
        })
    }
}

fn known_summary_outcome() -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Known,
        summary: Some(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        }),
        reason: None,
    }
}

fn mk_runtime(
    shell: Arc<dyn sieve_shell::ShellAnalyzer>,
    summaries: Arc<dyn CommandSummarizer>,
    policy_toml: &str,
    planner_output: PlannerTurnOutput,
    mainline: Arc<dyn MainlineRunner>,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<InProcessApprovalBus>,
    Arc<VecEventLog>,
) {
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell,
        summaries,
        policy: Arc::new(TomlPolicyEngine::from_toml_str(policy_toml).expect("policy parse")),
        quarantine: Arc::new(RecordingQuarantine::default()) as Arc<dyn QuarantineRunner>,
        mainline,
        web_search: Arc::new(NoopWebSearch),
        planner: Arc::new(StaticPlanner::new(planner_output)),
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone() as Arc<dyn RuntimeEventLog>,
        clock: Arc::new(SystemClock),
    }));
    (runtime, approval_bus, event_log)
}

#[tokio::test]
async fn l_disallowed_planner_tool_is_rejected_at_runtime_boundary() {
    let mut args = BTreeMap::new();
    args.insert("cmd".to_string(), json!("echo ok"));
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: vec![PlannerToolCall {
            tool_name: "bash".to_string(),
            args,
        }],
    };
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let (runtime, _approval_bus, _event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Known,
                segments,
                unsupported_constructs: Vec::new(),
            },
        }),
        Arc::new(StaticSummaries {
            outcome: known_summary_outcome(),
        }),
        BASE_POLICY,
        planner_output,
        Arc::new(NoopMainline),
    );

    let err = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-l".to_string()),
            cwd: "/tmp".to_string(),
            user_message: "run echo".to_string(),
            allowed_tools: vec!["endorse".to_string()],
            previous_events: Vec::new(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect_err("disallowed tool must fail");

    match err {
        RuntimeError::DisallowedTool {
            tool_call_index,
            tool_name,
            allowed_tools,
        } => {
            assert_eq!(tool_call_index, 0);
            assert_eq!(tool_name, "bash");
            assert_eq!(allowed_tools, vec!["endorse".to_string()]);
        }
        other => panic!("expected RuntimeError::DisallowedTool, got {other:?}"),
    }
}

#[tokio::test]
async fn m_endorse_policy_deny_blocks_approval_and_transition() {
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: Vec::new(),
    };
    let policy_toml = r#"
[[deny_rules]]
id = "deny-endorse"
argv_prefix = ["endorse"]
decision = "deny"
reason = "endorse disabled"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Known,
                segments,
                unsupported_constructs: Vec::new(),
            },
        }),
        Arc::new(StaticSummaries {
            outcome: known_summary_outcome(),
        }),
        policy_toml,
        planner_output,
        Arc::new(NoopMainline),
    );
    runtime
        .upsert_value_label(
            ValueRef("v_control".to_string()),
            sieve_types::ValueLabel {
                integrity: Integrity::Untrusted,
                provenance: BTreeSet::new(),
                allowed_sinks: BTreeSet::new(),
                capacity_type: sieve_types::CapacityType::TrustedString,
            },
        )
        .expect("seed value state");

    let transition = runtime
        .endorse_value_once(
            RunId("run-m".to_string()),
            EndorseRequest {
                value_ref: ValueRef("v_control".to_string()),
                target_integrity: Integrity::Trusted,
                reason: None,
            },
        )
        .await
        .expect("runtime ok");

    assert!(transition.is_none());
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
}

#[tokio::test]
async fn n_mainline_execution_runs_real_command_and_reports_exit_code() {
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: Vec::new(),
    };
    let root = std::env::temp_dir().join(format!("sieve-runtime-mainline-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir temp");
    let src = root.join("src.txt");
    let dst = root.join("dst.txt");
    fs::write(&src, "hello-mainline").expect("write src");

    let policy_toml = format!(
        r#"
[[allow_capabilities]]
resource = "fs"
action = "write"
scope = "{}"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        dst.to_string_lossy()
    );
    let (runtime, _approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        &policy_toml,
        planner_output,
        Arc::new(BashMainlineRunner),
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-n".to_string()),
            cwd: root.to_string_lossy().to_string(),
            script: "cp src.txt dst.txt".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-n".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(&dst).expect("read dst"),
        "hello-mainline"
    );
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn o_unknown_modes_deny_accept_ask_each_emit_policy_event() {
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: Vec::new(),
    };
    let segments = vec![CommandSegment {
        argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Unknown,
                segments,
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
        BASE_POLICY,
        planner_output,
        Arc::new(NoopMainline),
    );

    runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-o-deny".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("deny path");

    runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-o-accept".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Accept,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("accept path");

    let ask_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-o-ask".to_string()),
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
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");
    ask_task
        .await
        .expect("join ask")
        .expect("ask path runtime ok");

    let events = event_log.snapshot();
    let policy_events = events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_)))
        .count();
    assert_eq!(policy_events, 3);
}

#[tokio::test]
async fn o_uncertain_modes_deny_accept_ask_each_emit_policy_event() {
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: Vec::new(),
    };
    let segments = vec![CommandSegment {
        argv: vec!["weird-shell-construct".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Uncertain,
                segments,
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
        BASE_POLICY,
        planner_output,
        Arc::new(NoopMainline),
    );

    runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-o2-deny".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("deny path");

    runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-o2-accept".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Accept,
        })
        .await
        .expect("accept path");

    let ask_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-o2-ask".to_string()),
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
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2001,
        })
        .expect("resolve approval");
    ask_task
        .await
        .expect("join ask")
        .expect("ask path runtime ok");

    let events = event_log.snapshot();
    let policy_events = events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_)))
        .count();
    assert_eq!(policy_events, 3);
}
