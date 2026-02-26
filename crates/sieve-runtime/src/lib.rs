#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, SummaryOutcome};
use sieve_llm::PlannerModel;
use sieve_policy::PolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
use sieve_types::{
    ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, ApprovalResolvedEvent,
    CommandKnowledge, CommandSegment, CommandSummary, DeclassifyRequest, EndorseRequest,
    PolicyDecisionKind, PolicyEvaluatedEvent, PrecheckInput, QuarantineCompletedEvent,
    QuarantineReport, QuarantineRunRequest, RunId, RuntimeEvent, UncertainMode, UnknownMode,
};
use std::collections::HashMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::oneshot;

#[derive(Debug, Error)]
pub enum ApprovalBusError {
    #[error("approval transport failed: {0}")]
    Transport(String),
}

#[async_trait]
pub trait ApprovalBus: Send + Sync {
    async fn publish_requested(
        &self,
        event: ApprovalRequestedEvent,
    ) -> Result<(), ApprovalBusError>;

    async fn wait_resolved(
        &self,
        request_id: &ApprovalRequestId,
    ) -> Result<ApprovalResolvedEvent, ApprovalBusError>;
}

#[derive(Debug, Error)]
pub enum EventLogError {
    #[error("failed to append runtime event: {0}")]
    Append(String),
}

#[async_trait]
pub trait RuntimeEventLog: Send + Sync {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError>;
}

#[derive(Default)]
struct ApprovalState {
    senders: HashMap<ApprovalRequestId, oneshot::Sender<ApprovalResolvedEvent>>,
    receivers: HashMap<ApprovalRequestId, oneshot::Receiver<ApprovalResolvedEvent>>,
    published: Vec<ApprovalRequestedEvent>,
}

pub struct InProcessApprovalBus {
    state: Mutex<ApprovalState>,
}

impl InProcessApprovalBus {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ApprovalState::default()),
        }
    }

    pub fn resolve(&self, event: ApprovalResolvedEvent) -> Result<(), ApprovalBusError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApprovalBusError::Transport("approval state lock poisoned".to_string()))?;
        let Some(sender) = state.senders.remove(&event.request_id) else {
            return Err(ApprovalBusError::Transport(format!(
                "missing pending approval request: {}",
                event.request_id.0
            )));
        };
        sender
            .send(event)
            .map_err(|_| ApprovalBusError::Transport("approval receiver dropped".to_string()))
    }

    pub fn published_events(&self) -> Result<Vec<ApprovalRequestedEvent>, ApprovalBusError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ApprovalBusError::Transport("approval state lock poisoned".to_string()))?;
        Ok(state.published.clone())
    }
}

impl Default for InProcessApprovalBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApprovalBus for InProcessApprovalBus {
    async fn publish_requested(
        &self,
        event: ApprovalRequestedEvent,
    ) -> Result<(), ApprovalBusError> {
        let (sender, receiver) = oneshot::channel();
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApprovalBusError::Transport("approval state lock poisoned".to_string()))?;
        if state.senders.contains_key(&event.request_id) {
            return Err(ApprovalBusError::Transport(format!(
                "duplicate approval request id: {}",
                event.request_id.0
            )));
        }
        state.senders.insert(event.request_id.clone(), sender);
        state.receivers.insert(event.request_id.clone(), receiver);
        state.published.push(event);
        Ok(())
    }

    async fn wait_resolved(
        &self,
        request_id: &ApprovalRequestId,
    ) -> Result<ApprovalResolvedEvent, ApprovalBusError> {
        let receiver = {
            let mut state = self.state.lock().map_err(|_| {
                ApprovalBusError::Transport("approval state lock poisoned".to_string())
            })?;
            state.receivers.remove(request_id).ok_or_else(|| {
                ApprovalBusError::Transport(format!("missing approval receiver: {}", request_id.0))
            })?
        };

        receiver
            .await
            .map_err(|_| ApprovalBusError::Transport("approval sender dropped".to_string()))
    }
}

pub struct JsonlRuntimeEventLog {
    path: PathBuf,
    writer_lock: Mutex<()>,
}

impl JsonlRuntimeEventLog {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, EventLogError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            create_dir_all(parent).map_err(|err| EventLogError::Append(err.to_string()))?;
        }
        Ok(Self {
            path,
            writer_lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl RuntimeEventLog for JsonlRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        let _guard = self
            .writer_lock
            .lock()
            .map_err(|_| EventLogError::Append("event writer lock poisoned".to_string()))?;
        let encoded =
            serde_json::to_string(&event).map_err(|err| EventLogError::Append(err.to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| EventLogError::Append(err.to_string()))?;
        file.write_all(encoded.as_bytes())
            .map_err(|err| EventLogError::Append(err.to_string()))?;
        file.write_all(b"\n")
            .map_err(|err| EventLogError::Append(err.to_string()))?;
        Ok(())
    }
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("shell analysis failed: {0}")]
    Shell(#[from] ShellAnalysisError),
    #[error("runtime event log failed: {0}")]
    EventLog(#[from] EventLogError),
    #[error("approval bus failed: {0}")]
    Approval(#[from] ApprovalBusError),
    #[error("quarantine run failed: {0}")]
    Quarantine(#[from] QuarantineRunError),
}

#[derive(Debug, Clone)]
pub struct ShellRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub script: String,
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDisposition {
    ExecuteMainline,
    ExecuteQuarantine(QuarantineReport),
    Denied { reason: String },
}

pub struct RuntimeOrchestrator {
    shell: Arc<dyn ShellAnalyzer>,
    summaries: Arc<dyn CommandSummarizer>,
    policy: Arc<dyn PolicyEngine>,
    quarantine: Arc<dyn QuarantineRunner>,
    _planner: Arc<dyn PlannerModel>,
    approval_bus: Arc<dyn ApprovalBus>,
    event_log: Arc<dyn RuntimeEventLog>,
    clock: Arc<dyn Clock>,
    next_request: AtomicU64,
}

pub struct RuntimeDeps {
    pub shell: Arc<dyn ShellAnalyzer>,
    pub summaries: Arc<dyn CommandSummarizer>,
    pub policy: Arc<dyn PolicyEngine>,
    pub quarantine: Arc<dyn QuarantineRunner>,
    pub planner: Arc<dyn PlannerModel>,
    pub approval_bus: Arc<dyn ApprovalBus>,
    pub event_log: Arc<dyn RuntimeEventLog>,
    pub clock: Arc<dyn Clock>,
}

impl RuntimeOrchestrator {
    pub fn new(deps: RuntimeDeps) -> Self {
        Self {
            shell: deps.shell,
            summaries: deps.summaries,
            policy: deps.policy,
            quarantine: deps.quarantine,
            _planner: deps.planner,
            approval_bus: deps.approval_bus,
            event_log: deps.event_log,
            clock: deps.clock,
            next_request: AtomicU64::new(1),
        }
    }

    pub async fn orchestrate_shell(
        &self,
        request: ShellRunRequest,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let shell = self.shell.analyze_shell_lc_script(&request.script)?;
        let (knowledge, summary) = self.merge_summary(&shell.segments, shell.knowledge);

        if knowledge == CommandKnowledge::Unknown {
            return self
                .handle_unknown_or_uncertain(
                    &request,
                    shell.segments,
                    UnknownOrUncertain::Unknown,
                    Mode::from(request.unknown_mode),
                )
                .await;
        }

        if knowledge == CommandKnowledge::Uncertain {
            return self
                .handle_unknown_or_uncertain(
                    &request,
                    shell.segments,
                    UnknownOrUncertain::Uncertain,
                    Mode::from(request.uncertain_mode),
                )
                .await;
        }

        let inferred_capabilities = summary
            .as_ref()
            .map(|merged| merged.required_capabilities.clone())
            .unwrap_or_default();
        let precheck = PrecheckInput {
            run_id: request.run_id.clone(),
            cwd: request.cwd.clone(),
            command_segments: shell.segments.clone(),
            knowledge,
            summary,
            unknown_mode: request.unknown_mode,
            uncertain_mode: request.uncertain_mode,
        };

        let decision = self.policy.evaluate_precheck(&precheck);
        let policy_event = PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: request.run_id.clone(),
            decision: decision.clone(),
            inferred_capabilities: inferred_capabilities.clone(),
            trace_path: None,
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::PolicyEvaluated(policy_event))
            .await?;

        match decision.kind {
            PolicyDecisionKind::Allow => Ok(RuntimeDisposition::ExecuteMainline),
            PolicyDecisionKind::Deny => Ok(RuntimeDisposition::Denied {
                reason: decision.reason,
            }),
            PolicyDecisionKind::DenyWithApproval => {
                let blocked_rule_id = decision
                    .blocked_rule_id
                    .unwrap_or_else(|| "deny_with_approval".to_string());
                self.resolve_approval(
                    request.run_id,
                    precheck.command_segments,
                    inferred_capabilities,
                    blocked_rule_id,
                    decision.reason,
                    RuntimeDisposition::ExecuteMainline,
                )
                .await
            }
        }
    }

    pub async fn request_endorse_approval(
        &self,
        run_id: RunId,
        request: EndorseRequest,
    ) -> Result<ApprovalAction, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "endorse".to_string(),
                request.value_ref.0,
                format!("{:?}", request.target_integrity).to_lowercase(),
            ],
            operator_before: None,
        };
        self.approve_tool_call(
            run_id,
            segment,
            "endorse_requires_approval",
            "endorse requires approval",
        )
        .await
    }

    pub async fn request_declassify_approval(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
    ) -> Result<ApprovalAction, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "declassify".to_string(),
                request.value_ref.0,
                request.sink.0,
            ],
            operator_before: None,
        };
        self.approve_tool_call(
            run_id,
            segment,
            "declassify_requires_approval",
            "declassify requires approval",
        )
        .await
    }

    fn merge_summary(
        &self,
        segments: &[CommandSegment],
        shell_knowledge: CommandKnowledge,
    ) -> (CommandKnowledge, Option<CommandSummary>) {
        if shell_knowledge != CommandKnowledge::Known {
            return (shell_knowledge, None);
        }

        let mut merged = CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        };

        for segment in segments {
            let SummaryOutcome {
                knowledge, summary, ..
            } = self.summaries.summarize(&segment.argv);
            match knowledge {
                CommandKnowledge::Known => {
                    let Some(summary) = summary else {
                        return (CommandKnowledge::Unknown, None);
                    };
                    merged
                        .required_capabilities
                        .extend(summary.required_capabilities);
                    merged.sink_checks.extend(summary.sink_checks);
                    merged.unsupported_flags.extend(summary.unsupported_flags);
                }
                CommandKnowledge::Unknown => return (CommandKnowledge::Unknown, None),
                CommandKnowledge::Uncertain => return (CommandKnowledge::Uncertain, None),
            }
        }

        (CommandKnowledge::Known, Some(merged))
    }

    async fn handle_unknown_or_uncertain(
        &self,
        request: &ShellRunRequest,
        segments: Vec<CommandSegment>,
        kind: UnknownOrUncertain,
        mode: Mode,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        match mode {
            Mode::Deny => Ok(RuntimeDisposition::Denied {
                reason: kind.to_deny_reason().to_string(),
            }),
            Mode::Ask => {
                let action = self
                    .request_approval(
                        request.run_id.clone(),
                        segments.clone(),
                        Vec::new(),
                        kind.to_blocked_rule_id().to_string(),
                        kind.to_approval_reason().to_string(),
                    )
                    .await?;
                match action {
                    ApprovalAction::ApproveOnce => {
                        let report = self
                            .run_quarantine(request.run_id.clone(), request.cwd.clone(), segments)
                            .await?;
                        Ok(RuntimeDisposition::ExecuteQuarantine(report))
                    }
                    ApprovalAction::Deny => Ok(RuntimeDisposition::Denied {
                        reason: "approval denied".to_string(),
                    }),
                }
            }
            Mode::Accept => {
                let report = self
                    .run_quarantine(request.run_id.clone(), request.cwd.clone(), segments)
                    .await?;
                Ok(RuntimeDisposition::ExecuteQuarantine(report))
            }
        }
    }

    async fn run_quarantine(
        &self,
        run_id: RunId,
        cwd: String,
        command_segments: Vec<CommandSegment>,
    ) -> Result<QuarantineReport, RuntimeError> {
        let report = self
            .quarantine
            .run(QuarantineRunRequest {
                run_id: run_id.clone(),
                cwd,
                command_segments,
            })
            .await?;
        let quarantine_event = QuarantineCompletedEvent {
            schema_version: 1,
            run_id,
            report: report.clone(),
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::QuarantineCompleted(quarantine_event))
            .await?;
        Ok(report)
    }

    async fn approve_tool_call(
        &self,
        run_id: RunId,
        segment: CommandSegment,
        blocked_rule_id: &str,
        reason: &str,
    ) -> Result<ApprovalAction, RuntimeError> {
        self.request_approval(
            run_id,
            vec![segment],
            Vec::new(),
            blocked_rule_id.to_string(),
            reason.to_string(),
        )
        .await
    }

    async fn resolve_approval(
        &self,
        run_id: RunId,
        command_segments: Vec<CommandSegment>,
        inferred_capabilities: Vec<sieve_types::Capability>,
        blocked_rule_id: String,
        reason: String,
        approved: RuntimeDisposition,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        match self
            .request_approval(
                run_id,
                command_segments,
                inferred_capabilities,
                blocked_rule_id,
                reason,
            )
            .await?
        {
            ApprovalAction::ApproveOnce => Ok(approved),
            ApprovalAction::Deny => Ok(RuntimeDisposition::Denied {
                reason: "approval denied".to_string(),
            }),
        }
    }

    async fn request_approval(
        &self,
        run_id: RunId,
        command_segments: Vec<CommandSegment>,
        inferred_capabilities: Vec<sieve_types::Capability>,
        blocked_rule_id: String,
        reason: String,
    ) -> Result<ApprovalAction, RuntimeError> {
        let request_id = self.new_request_id();
        let approval_requested = ApprovalRequestedEvent {
            schema_version: 1,
            request_id: request_id.clone(),
            run_id,
            command_segments,
            inferred_capabilities,
            blocked_rule_id,
            reason,
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::ApprovalRequested(approval_requested.clone()))
            .await?;
        self.approval_bus
            .publish_requested(approval_requested)
            .await?;

        let approval_resolved = self.approval_bus.wait_resolved(&request_id).await?;
        self.append_event(RuntimeEvent::ApprovalResolved(approval_resolved.clone()))
            .await?;
        Ok(approval_resolved.action)
    }

    async fn append_event(&self, event: RuntimeEvent) -> Result<(), RuntimeError> {
        self.event_log.append(event).await?;
        Ok(())
    }

    fn new_request_id(&self) -> ApprovalRequestId {
        let next = self.next_request.fetch_add(1, Ordering::Relaxed);
        ApprovalRequestId(format!("approval-{next}"))
    }
}

#[derive(Clone, Copy)]
enum UnknownOrUncertain {
    Unknown,
    Uncertain,
}

impl UnknownOrUncertain {
    fn to_blocked_rule_id(self) -> &'static str {
        match self {
            Self::Unknown => "unknown_command_mode",
            Self::Uncertain => "uncertain_command_mode",
        }
    }

    fn to_approval_reason(self) -> &'static str {
        match self {
            Self::Unknown => "unknown command requires approval",
            Self::Uncertain => "uncertain command requires approval",
        }
    }

    fn to_deny_reason(self) -> &'static str {
        match self {
            Self::Unknown => "unknown command denied by mode",
            Self::Uncertain => "uncertain command denied by mode",
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Ask,
    Accept,
    Deny,
}

impl From<UnknownMode> for Mode {
    fn from(value: UnknownMode) -> Self {
        match value {
            UnknownMode::Ask => Self::Ask,
            UnknownMode::Accept => Self::Accept,
            UnknownMode::Deny => Self::Deny,
        }
    }
}

impl From<UncertainMode> for Mode {
    fn from(value: UncertainMode) -> Self {
        match value {
            UncertainMode::Ask => Self::Ask,
            UncertainMode::Accept => Self::Accept,
            UncertainMode::Deny => Self::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_llm::LlmError;
    use sieve_shell::ShellAnalysis;
    use sieve_types::{
        Action, Capability, CommandKnowledge, CommandSummary, LlmModelConfig, LlmProvider,
        PlannerTurnInput, PlannerTurnOutput, PolicyDecision, QuarantineExtractInput,
        QuarantineExtractOutput, Resource, SinkCheck, SinkKey, TypedValue, ValueRef,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::env::temp_dir;
    use std::fs::{read_to_string, remove_file};
    use std::sync::Mutex as StdMutex;
    use tokio::time::{sleep, Duration};

    struct StubShell {
        analysis: ShellAnalysis,
    }

    impl ShellAnalyzer for StubShell {
        fn analyze_shell_lc_script(
            &self,
            _script: &str,
        ) -> Result<ShellAnalysis, ShellAnalysisError> {
            Ok(self.analysis.clone())
        }
    }

    struct StubSummaries {
        outcome: SummaryOutcome,
    }

    impl CommandSummarizer for StubSummaries {
        fn summarize(&self, _argv: &[String]) -> SummaryOutcome {
            self.outcome.clone()
        }
    }

    struct StubPolicy {
        decision: PolicyDecision,
    }

    impl PolicyEngine for StubPolicy {
        fn evaluate_precheck(&self, _input: &PrecheckInput) -> PolicyDecision {
            self.decision.clone()
        }
    }

    struct StubQuarantine {
        report: QuarantineReport,
    }

    #[async_trait]
    impl QuarantineRunner for StubQuarantine {
        async fn run(
            &self,
            _request: QuarantineRunRequest,
        ) -> Result<QuarantineReport, QuarantineRunError> {
            Ok(self.report.clone())
        }
    }

    struct StubPlanner {
        config: LlmModelConfig,
    }

    #[async_trait]
    impl PlannerModel for StubPlanner {
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

    struct DeterministicClock {
        now: AtomicU64,
    }

    impl DeterministicClock {
        fn new(start: u64) -> Self {
            Self {
                now: AtomicU64::new(start),
            }
        }
    }

    impl Clock for DeterministicClock {
        fn now_ms(&self) -> u64 {
            self.now.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Default)]
    struct VecEventLog {
        events: StdMutex<Vec<RuntimeEvent>>,
    }

    #[async_trait]
    impl RuntimeEventLog for VecEventLog {
        async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
            self.events
                .lock()
                .map_err(|_| EventLogError::Append("test lock poisoned".to_string()))?
                .push(event);
            Ok(())
        }
    }

    impl VecEventLog {
        fn snapshot(&self) -> Vec<RuntimeEvent> {
            self.events.lock().expect("event lock").clone()
        }
    }

    fn stub_summary() -> CommandSummary {
        CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/test".to_string(),
            }],
            sink_checks: vec![SinkCheck {
                argument_name: "body".to_string(),
                sink: SinkKey("https://example.com/path".to_string()),
                value_refs: vec![ValueRef("v1".to_string())],
            }],
            unsupported_flags: Vec::new(),
        }
    }

    fn mk_runtime(
        shell_knowledge: CommandKnowledge,
        segments: Vec<CommandSegment>,
        summary_knowledge: CommandKnowledge,
        policy_kind: PolicyDecisionKind,
    ) -> (
        Arc<RuntimeOrchestrator>,
        Arc<InProcessApprovalBus>,
        Arc<VecEventLog>,
    ) {
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(VecEventLog::default());
        let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
            shell: Arc::new(StubShell {
                analysis: ShellAnalysis {
                    knowledge: shell_knowledge,
                    segments,
                    unsupported_constructs: Vec::new(),
                },
            }),
            summaries: Arc::new(StubSummaries {
                outcome: SummaryOutcome {
                    knowledge: summary_knowledge,
                    summary: if summary_knowledge == CommandKnowledge::Known {
                        Some(stub_summary())
                    } else {
                        None
                    },
                    reason: None,
                },
            }),
            policy: Arc::new(StubPolicy {
                decision: PolicyDecision {
                    kind: policy_kind,
                    reason: "policy verdict".to_string(),
                    blocked_rule_id: Some("rule-1".to_string()),
                },
            }),
            quarantine: Arc::new(StubQuarantine {
                report: QuarantineReport {
                    run_id: RunId("run-1".to_string()),
                    trace_path: "/tmp/sieve/trace".to_string(),
                    stdout_path: None,
                    stderr_path: None,
                    attempted_capabilities: Vec::new(),
                    exit_code: Some(0),
                },
            }),
            planner: Arc::new(StubPlanner {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "gpt-test".to_string(),
                    api_base: None,
                },
            }),
            approval_bus: approval_bus.clone(),
            event_log: event_log.clone(),
            clock: Arc::new(DeterministicClock::new(1000)),
        }));
        (runtime, approval_bus, event_log)
    }

    async fn wait_for_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
        for _ in 0..20 {
            let published = bus.published_events().expect("published events");
            if let Some(first) = published.first() {
                return first.clone();
            }
            sleep(Duration::from_millis(5)).await;
        }
        panic!("approval not requested in time");
    }

    #[tokio::test]
    async fn approval_roundtrip_known_command() {
        let segments = vec![CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::DenyWithApproval,
        );

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run-1".to_string()),
                        cwd: "/tmp".to_string(),
                        script: "rm -rf tmp".to_string(),
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id.clone(),
                run_id: requested.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let disposition = runtime_task.await.expect("task join").expect("runtime ok");
        assert_eq!(disposition, RuntimeDisposition::ExecuteMainline);

        let events = event_log.snapshot();
        assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
        assert!(matches!(events[1], RuntimeEvent::ApprovalRequested(_)));
        assert!(matches!(events[2], RuntimeEvent::ApprovalResolved(_)));
        match &events[0] {
            RuntimeEvent::PolicyEvaluated(event) => assert_eq!(event.created_at_ms, 1000),
            _ => panic!("expected policy evaluated event"),
        }
        match &events[1] {
            RuntimeEvent::ApprovalRequested(event) => assert_eq!(event.created_at_ms, 1001),
            _ => panic!("expected approval requested event"),
        }
    }

    #[tokio::test]
    async fn composed_command_consolidates_single_approval() {
        let segments = vec![
            CommandSegment {
                argv: vec!["echo".to_string(), "hi".to_string()],
                operator_before: None,
            },
            CommandSegment {
                argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
                operator_before: Some(sieve_types::CompositionOperator::And),
            },
        ];

        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::DenyWithApproval,
        );

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run-1".to_string()),
                        cwd: "/tmp".to_string(),
                        script: "echo hi && rm -rf tmp".to_string(),
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.command_segments.len(), 2);

        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id,
                run_id: RunId("run-1".to_string()),
                action: ApprovalAction::Deny,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let disposition = runtime_task.await.expect("task join").expect("runtime ok");
        assert_eq!(
            disposition,
            RuntimeDisposition::Denied {
                reason: "approval denied".to_string()
            }
        );
    }

    #[tokio::test]
    async fn unknown_ask_requires_approval_before_quarantine() {
        let segments = vec![CommandSegment {
            argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, event_log) = mk_runtime(
            CommandKnowledge::Unknown,
            segments,
            CommandKnowledge::Unknown,
            PolicyDecisionKind::Allow,
        );

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run-1".to_string()),
                        cwd: "/tmp".to_string(),
                        script: "custom-cmd --flag".to_string(),
                        unknown_mode: UnknownMode::Ask,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.blocked_rule_id, "unknown_command_mode");

        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id,
                run_id: RunId("run-1".to_string()),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let disposition = runtime_task.await.expect("task join").expect("runtime ok");
        assert!(matches!(
            disposition,
            RuntimeDisposition::ExecuteQuarantine(_)
        ));

        let events = event_log.snapshot();
        assert!(matches!(events[0], RuntimeEvent::ApprovalRequested(_)));
        assert!(matches!(events[1], RuntimeEvent::ApprovalResolved(_)));
        assert!(matches!(events[2], RuntimeEvent::QuarantineCompleted(_)));
    }

    #[tokio::test]
    async fn endorse_request_lifecycle_uses_approval_flow() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .request_endorse_approval(
                        RunId("run-1".to_string()),
                        EndorseRequest {
                            value_ref: ValueRef("v123".to_string()),
                            target_integrity: sieve_types::Integrity::Trusted,
                            reason: None,
                        },
                    )
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.command_segments[0].argv[0], "endorse");

        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id,
                run_id: RunId("run-1".to_string()),
                action: ApprovalAction::Deny,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let action = runtime_task.await.expect("task join").expect("runtime ok");
        assert_eq!(action, ApprovalAction::Deny);

        let events = event_log.snapshot();
        assert!(matches!(events[0], RuntimeEvent::ApprovalRequested(_)));
        assert!(matches!(events[1], RuntimeEvent::ApprovalResolved(_)));
    }

    #[tokio::test]
    async fn declassify_request_lifecycle_uses_approval_flow() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .request_declassify_approval(
                        RunId("run-1".to_string()),
                        DeclassifyRequest {
                            value_ref: ValueRef("v456".to_string()),
                            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                            reason: None,
                        },
                    )
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.command_segments[0].argv[0], "declassify");
        assert_eq!(
            requested.command_segments[0].argv[2],
            "https://api.example.com/v1/upload"
        );

        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id,
                run_id: RunId("run-1".to_string()),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let action = runtime_task.await.expect("task join").expect("runtime ok");
        assert_eq!(action, ApprovalAction::ApproveOnce);

        let events = event_log.snapshot();
        assert!(matches!(events[0], RuntimeEvent::ApprovalRequested(_)));
        assert!(matches!(events[1], RuntimeEvent::ApprovalResolved(_)));
    }

    #[tokio::test]
    async fn jsonl_event_log_appends_in_order() {
        let path = temp_dir().join(format!("sieve-runtime-events-{}.jsonl", std::process::id()));
        let _ = remove_file(&path);
        let log = JsonlRuntimeEventLog::new(&path).expect("create log");

        log.append(RuntimeEvent::ApprovalRequested(ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-1".to_string()),
            run_id: RunId("run-1".to_string()),
            command_segments: vec![CommandSegment {
                argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
                operator_before: None,
            }],
            inferred_capabilities: Vec::new(),
            blocked_rule_id: "rule-1".to_string(),
            reason: "needs approval".to_string(),
            created_at_ms: 1000,
        }))
        .await
        .expect("append request");

        log.append(RuntimeEvent::ApprovalResolved(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-1".to_string()),
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1001,
        }))
        .await
        .expect("append resolution");

        let body = read_to_string(&path).expect("read log file");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: RuntimeEvent = serde_json::from_str(lines[0]).expect("parse first event");
        let second: RuntimeEvent = serde_json::from_str(lines[1]).expect("parse second event");
        assert!(matches!(first, RuntimeEvent::ApprovalRequested(_)));
        assert!(matches!(second, RuntimeEvent::ApprovalResolved(_)));

        let _ = remove_file(path);
    }

    #[allow(dead_code)]
    async fn _unused_quarantine_extract_example(
        _input: QuarantineExtractInput,
    ) -> QuarantineExtractOutput {
        QuarantineExtractOutput {
            value: TypedValue::Enum {
                registry: "r".to_string(),
                variant: "v".to_string(),
            },
        }
    }

    #[allow(dead_code)]
    fn _unused_map_example() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::new()
    }
}
