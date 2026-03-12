#![forbid(unsafe_code)]
#![allow(dead_code)]

use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, SummaryOutcome};
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_runtime::{
    Clock, EventLogError, InProcessApprovalBus, MainlineRunError, MainlineRunReport,
    MainlineRunRequest, MainlineRunner, RuntimeDeps, RuntimeEventLog, RuntimeOrchestrator,
};
use sieve_shell::{ShellAnalysis, ShellAnalysisError, ShellAnalyzer};
use sieve_types::{
    ApprovalRequestedEvent, CapacityType, Integrity, LlmModelConfig, LlmProvider, PlannerTurnInput,
    PlannerTurnOutput, QuarantineReport, QuarantineRunRequest, RuntimeEvent, SinkKey, Source,
    ValueLabel,
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
pub struct VecEventLog {
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
    pub fn snapshot(&self) -> Vec<RuntimeEvent> {
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

pub struct StaticShell {
    pub analysis: ShellAnalysis,
}

impl ShellAnalyzer for StaticShell {
    fn analyze_shell_lc_script(&self, _script: &str) -> Result<ShellAnalysis, ShellAnalysisError> {
        Ok(self.analysis.clone())
    }
}

pub struct StaticSummaries {
    pub outcome: SummaryOutcome,
}

impl CommandSummarizer for StaticSummaries {
    fn summarize(&self, _argv: &[String]) -> SummaryOutcome {
        self.outcome.clone()
    }
}

#[derive(Default)]
pub struct RecordingQuarantine {
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
    pub fn calls(&self) -> Vec<QuarantineRunRequest> {
        self.calls.lock().expect("calls lock").clone()
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

pub fn mk_runtime(
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
        mainline: Arc::new(NoopMainline),
        planner,
        automation: None,
        codex: None,
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(TestClock::new(1000)),
    }));
    (runtime, approval_bus, event_log)
}

pub fn label_with_sinks(integrity: Integrity, sinks: &[&str]) -> ValueLabel {
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

pub async fn wait_for_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
    for _ in 0..30 {
        let published = bus.published_events().expect("published events");
        if let Some(first) = published.first() {
            return first.clone();
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("approval not requested in time");
}

pub async fn wait_for_approval_count(
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

pub fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}

pub fn write_fake_bwrap(path: &Path) {
    fs::write(
        path,
        "#!/usr/bin/env bash\nset -euo pipefail\ntrace_base=\"\"\nfor ((i=1;i<=${#};i++)); do\n  arg=\"${!i}\"\n  if [[ \"$arg\" == \"-o\" ]]; then\n    next=$((i+1))\n    trace_base=\"${!next}\"\n  fi\ndone\necho 'execve(\"/bin/echo\", [\"echo\"], 0x0) = 0' > \"${trace_base}.123\"\necho fake-stdout\necho fake-stderr >&2\n",
    )
    .expect("write fake bwrap");
    let mut perms = fs::metadata(path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod fake bwrap");
}
