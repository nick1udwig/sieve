#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, SummaryOutcome};
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::PolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
use sieve_tool_contracts::{validate_at_index, TypedCall, TOOL_CONTRACTS_VERSION};
use sieve_types::{
    ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, ApprovalResolvedEvent,
    CommandKnowledge, CommandSegment, CommandSummary, ControlContext, DeclassifyRequest,
    DeclassifyStateTransition, EndorseRequest, EndorseStateTransition, Integrity, PlannerToolCall,
    PlannerTurnInput, PolicyDecision, PolicyDecisionKind, PolicyEvaluatedEvent, PrecheckInput,
    QuarantineCompletedEvent, QuarantineReport, QuarantineRunRequest, RunId, RuntimeEvent,
    RuntimePolicyContext, SinkKey, SinkPermissionContext, ToolContractValidationReport,
    UncertainMode, UnknownMode, ValueLabel, ValueRef,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
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

#[derive(Debug, Error)]
pub enum ValueStateError {
    #[error("value state lock poisoned")]
    LockPoisoned,
    #[error("unknown value reference: {0}")]
    UnknownValueRef(String),
}

#[derive(Default)]
struct RuntimeValueState {
    labels_by_value: BTreeMap<ValueRef, ValueLabel>,
}

impl RuntimeValueState {
    fn upsert_label(&mut self, value_ref: ValueRef, label: ValueLabel) {
        self.labels_by_value.insert(value_ref, label);
    }

    fn runtime_policy_context_for_control(
        &self,
        control_value_refs: BTreeSet<ValueRef>,
        endorsed_by: Option<ApprovalRequestId>,
    ) -> RuntimePolicyContext {
        let control_integrity = if control_value_refs.is_empty()
            || control_value_refs.iter().all(|value_ref| {
                self.labels_by_value
                    .get(value_ref)
                    .map(|label| label.integrity == Integrity::Trusted)
                    .unwrap_or(false)
            }) {
            Integrity::Trusted
        } else {
            Integrity::Untrusted
        };

        let allowed_sinks_by_value = self
            .labels_by_value
            .iter()
            .map(|(value_ref, label)| (value_ref.clone(), label.allowed_sinks.clone()))
            .collect();

        RuntimePolicyContext {
            control: ControlContext {
                integrity: control_integrity,
                value_refs: control_value_refs,
                endorsed_by,
            },
            sink_permissions: SinkPermissionContext {
                allowed_sinks_by_value,
            },
        }
    }

    fn apply_endorse_transition(
        &mut self,
        value_ref: ValueRef,
        to_integrity: Integrity,
        approved_by: Option<ApprovalRequestId>,
    ) -> Result<EndorseStateTransition, ValueStateError> {
        let label = self
            .labels_by_value
            .get_mut(&value_ref)
            .ok_or_else(|| ValueStateError::UnknownValueRef(value_ref.0.clone()))?;
        let from_integrity = label.integrity;
        label.integrity = to_integrity;

        Ok(EndorseStateTransition {
            value_ref,
            from_integrity,
            to_integrity,
            approved_by,
        })
    }

    fn apply_declassify_transition(
        &mut self,
        value_ref: ValueRef,
        sink: SinkKey,
        approved_by: Option<ApprovalRequestId>,
    ) -> Result<DeclassifyStateTransition, ValueStateError> {
        let label = self
            .labels_by_value
            .get_mut(&value_ref)
            .ok_or_else(|| ValueStateError::UnknownValueRef(value_ref.0.clone()))?;
        let sink_was_already_allowed = !label.allowed_sinks.insert(sink.clone());

        Ok(DeclassifyStateTransition {
            value_ref,
            sink,
            sink_was_already_allowed,
            approved_by,
        })
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub script: String,
    pub command_segments: Vec<CommandSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineRunReport {
    pub run_id: RunId,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Error)]
pub enum MainlineRunError {
    #[error("mainline command execution failed: {0}")]
    Exec(String),
}

#[async_trait]
pub trait MainlineRunner: Send + Sync {
    async fn run(&self, request: MainlineRunRequest)
        -> Result<MainlineRunReport, MainlineRunError>;
}

pub struct BashMainlineRunner;

#[async_trait]
impl MainlineRunner for BashMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        let status = StdCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .status()
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: status.code(),
        })
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("planner model failed: {0}")]
    Planner(#[from] LlmError),
    #[error("shell analysis failed: {0}")]
    Shell(#[from] ShellAnalysisError),
    #[error("runtime event log failed: {0}")]
    EventLog(#[from] EventLogError),
    #[error("approval bus failed: {0}")]
    Approval(#[from] ApprovalBusError),
    #[error("quarantine run failed: {0}")]
    Quarantine(#[from] QuarantineRunError),
    #[error("mainline run failed: {0}")]
    Mainline(#[from] MainlineRunError),
    #[error("value state failed: {0}")]
    ValueState(#[from] ValueStateError),
    #[error("planner tool call contract validation failed")]
    ToolContract {
        report: ToolContractValidationReport,
    },
    #[error("planner emitted disallowed tool `{tool_name}` at index {tool_call_index}")]
    DisallowedTool {
        tool_call_index: usize,
        tool_name: String,
        allowed_tools: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ShellRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub script: String,
    pub control_value_refs: BTreeSet<ValueRef>,
    pub control_endorsed_by: Option<ApprovalRequestId>,
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}

#[derive(Debug, Clone)]
pub struct PlannerRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub user_message: String,
    pub allowed_tools: Vec<String>,
    pub previous_events: Vec<RuntimeEvent>,
    pub control_value_refs: BTreeSet<ValueRef>,
    pub control_endorsed_by: Option<ApprovalRequestId>,
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDisposition {
    ExecuteMainline(MainlineRunReport),
    ExecuteQuarantine(QuarantineReport),
    Denied { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannerToolResult {
    Bash {
        command: String,
        disposition: RuntimeDisposition,
    },
    Endorse {
        request: EndorseRequest,
        transition: Option<EndorseStateTransition>,
    },
    Declassify {
        request: DeclassifyRequest,
        transition: Option<DeclassifyStateTransition>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerRunResult {
    pub thoughts: Option<String>,
    pub tool_results: Vec<PlannerToolResult>,
}

pub struct RuntimeOrchestrator {
    shell: Arc<dyn ShellAnalyzer>,
    summaries: Arc<dyn CommandSummarizer>,
    policy: Arc<dyn PolicyEngine>,
    quarantine: Arc<dyn QuarantineRunner>,
    mainline: Arc<dyn MainlineRunner>,
    planner: Arc<dyn PlannerModel>,
    approval_bus: Arc<dyn ApprovalBus>,
    event_log: Arc<dyn RuntimeEventLog>,
    clock: Arc<dyn Clock>,
    next_request: AtomicU64,
    value_state: Mutex<RuntimeValueState>,
}

pub struct RuntimeDeps {
    pub shell: Arc<dyn ShellAnalyzer>,
    pub summaries: Arc<dyn CommandSummarizer>,
    pub policy: Arc<dyn PolicyEngine>,
    pub quarantine: Arc<dyn QuarantineRunner>,
    pub mainline: Arc<dyn MainlineRunner>,
    pub planner: Arc<dyn PlannerModel>,
    pub approval_bus: Arc<dyn ApprovalBus>,
    pub event_log: Arc<dyn RuntimeEventLog>,
    pub clock: Arc<dyn Clock>,
}

impl RuntimeOrchestrator {
    /// Constructs the runtime orchestrator from injected crate boundaries.
    /// Approval is one-shot and never mutates persistent policy.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self {
            shell: deps.shell,
            summaries: deps.summaries,
            policy: deps.policy,
            quarantine: deps.quarantine,
            mainline: deps.mainline,
            planner: deps.planner,
            approval_bus: deps.approval_bus,
            event_log: deps.event_log,
            clock: deps.clock,
            next_request: AtomicU64::new(1),
            value_state: Mutex::new(RuntimeValueState::default()),
        }
    }

    pub fn upsert_value_label(
        &self,
        value_ref: ValueRef,
        label: ValueLabel,
    ) -> Result<(), RuntimeError> {
        let mut state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        state.upsert_label(value_ref, label);
        Ok(())
    }

    pub fn value_label(&self, value_ref: &ValueRef) -> Result<Option<ValueLabel>, RuntimeError> {
        let state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(state.labels_by_value.get(value_ref).cloned())
    }

    pub fn runtime_policy_context_for_control(
        &self,
        control_value_refs: BTreeSet<ValueRef>,
        endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<RuntimePolicyContext, RuntimeError> {
        let state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(state.runtime_policy_context_for_control(control_value_refs, endorsed_by))
    }

    /// Runs a full planner turn and dispatches each validated tool call through runtime gates.
    pub async fn orchestrate_planner_turn(
        &self,
        request: PlannerRunRequest,
    ) -> Result<PlannerRunResult, RuntimeError> {
        let planner_output = self
            .planner
            .plan_turn(PlannerTurnInput {
                run_id: request.run_id.clone(),
                user_message: request.user_message.clone(),
                allowed_tools: request.allowed_tools.clone(),
                previous_events: request.previous_events.clone(),
            })
            .await?;

        let mut tool_results = Vec::with_capacity(planner_output.tool_calls.len());
        for (idx, tool_call) in planner_output.tool_calls.into_iter().enumerate() {
            Self::ensure_tool_allowed(idx, &tool_call.tool_name, &request.allowed_tools)?;
            let typed_call = self.validate_planner_tool_call(idx, &tool_call)?;
            match typed_call {
                TypedCall::Bash(args) => {
                    let disposition = self
                        .orchestrate_shell(ShellRunRequest {
                            run_id: request.run_id.clone(),
                            cwd: request.cwd.clone(),
                            script: args.cmd.clone(),
                            control_value_refs: request.control_value_refs.clone(),
                            control_endorsed_by: request.control_endorsed_by.clone(),
                            unknown_mode: request.unknown_mode,
                            uncertain_mode: request.uncertain_mode,
                        })
                        .await?;
                    tool_results.push(PlannerToolResult::Bash {
                        command: args.cmd,
                        disposition,
                    });
                }
                TypedCall::Endorse(endorse_request) => {
                    let transition = self
                        .endorse_value_once(request.run_id.clone(), endorse_request.clone())
                        .await?;
                    tool_results.push(PlannerToolResult::Endorse {
                        request: endorse_request,
                        transition,
                    });
                }
                TypedCall::Declassify(declassify_request) => {
                    let transition = self
                        .declassify_value_once(request.run_id.clone(), declassify_request.clone())
                        .await?;
                    tool_results.push(PlannerToolResult::Declassify {
                        request: declassify_request,
                        transition,
                    });
                }
            }
        }

        Ok(PlannerRunResult {
            thoughts: planner_output.thoughts,
            tool_results,
        })
    }

    /// Orchestrates shell execution precheck flow.
    /// For composed commands, this executes a single all-or-nothing precheck
    /// and emits one consolidated approval request when needed.
    pub async fn orchestrate_shell(
        &self,
        request: ShellRunRequest,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let runtime_context = self.runtime_policy_context_for_control(
            request.control_value_refs.clone(),
            request.control_endorsed_by.clone(),
        )?;
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
            runtime_context,
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

        let command_segments = precheck.command_segments;
        match decision.kind {
            PolicyDecisionKind::Allow => {
                self.execute_mainline(
                    request.run_id.clone(),
                    request.cwd.clone(),
                    request.script.clone(),
                    command_segments,
                )
                .await
            }
            PolicyDecisionKind::Deny => Ok(RuntimeDisposition::Denied {
                reason: decision.reason,
            }),
            PolicyDecisionKind::DenyWithApproval => {
                let blocked_rule_id = decision
                    .blocked_rule_id
                    .unwrap_or_else(|| "deny_with_approval".to_string());
                match self
                    .request_approval(
                        request.run_id.clone(),
                        command_segments.clone(),
                        inferred_capabilities,
                        blocked_rule_id,
                        decision.reason,
                    )
                    .await?
                    .action
                {
                    ApprovalAction::ApproveOnce => {
                        self.execute_mainline(
                            request.run_id,
                            request.cwd,
                            request.script,
                            command_segments,
                        )
                        .await
                    }
                    ApprovalAction::Deny => Ok(RuntimeDisposition::Denied {
                        reason: "approval denied".to_string(),
                    }),
                }
            }
        }
    }

    /// Starts approval lifecycle for an explicit `endorse` tool request.
    /// Returns one-shot approval action; no persistent allowlist state.
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
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "endorse_requires_approval",
                "endorse requires approval",
            )
            .await?
        else {
            return Ok(ApprovalAction::Deny);
        };

        Ok(resolution.action)
    }

    /// Runs `endorse` approval and applies state transition when approved once.
    pub async fn endorse_value_once(
        &self,
        run_id: RunId,
        request: EndorseRequest,
    ) -> Result<Option<EndorseStateTransition>, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "endorse".to_string(),
                request.value_ref.0.clone(),
                format!("{:?}", request.target_integrity).to_lowercase(),
            ],
            operator_before: None,
        };
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "endorse_requires_approval",
                "endorse requires approval",
            )
            .await?
        else {
            return Ok(None);
        };

        if resolution.action == ApprovalAction::Deny {
            return Ok(None);
        }

        let mut state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        let transition = state.apply_endorse_transition(
            request.value_ref,
            request.target_integrity,
            Some(resolution.request_id),
        )?;
        Ok(Some(transition))
    }

    /// Starts approval lifecycle for an explicit `declassify` tool request.
    /// Returns one-shot approval action; no persistent allowlist state.
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
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "declassify_requires_approval",
                "declassify requires approval",
            )
            .await?
        else {
            return Ok(ApprovalAction::Deny);
        };

        Ok(resolution.action)
    }

    /// Runs `declassify` approval and applies state transition when approved once.
    pub async fn declassify_value_once(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
    ) -> Result<Option<DeclassifyStateTransition>, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "declassify".to_string(),
                request.value_ref.0.clone(),
                request.sink.0.clone(),
            ],
            operator_before: None,
        };
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "declassify_requires_approval",
                "declassify requires approval",
            )
            .await?
        else {
            return Ok(None);
        };

        if resolution.action == ApprovalAction::Deny {
            return Ok(None);
        }

        let mut state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        let transition = state.apply_declassify_transition(
            request.value_ref,
            request.sink,
            Some(resolution.request_id),
        )?;
        Ok(Some(transition))
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

    fn validate_planner_tool_call(
        &self,
        tool_call_index: usize,
        tool_call: &PlannerToolCall,
    ) -> Result<TypedCall, RuntimeError> {
        let args_json = serde_json::Value::Object(
            tool_call
                .args
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        match validate_at_index(tool_call_index, &tool_call.tool_name, &args_json) {
            Ok(typed) => Ok(typed),
            Err(error) => {
                let report = ToolContractValidationReport {
                    contract_version: TOOL_CONTRACTS_VERSION,
                    errors: vec![error.as_validation_error()],
                };
                Self::log_tool_contract_failure(&report);
                Err(RuntimeError::ToolContract { report })
            }
        }
    }

    fn ensure_tool_allowed(
        tool_call_index: usize,
        tool_name: &str,
        allowed_tools: &[String],
    ) -> Result<(), RuntimeError> {
        if allowed_tools.iter().any(|allowed| allowed == tool_name) {
            return Ok(());
        }

        Err(RuntimeError::DisallowedTool {
            tool_call_index,
            tool_name: tool_name.to_string(),
            allowed_tools: allowed_tools.to_vec(),
        })
    }

    fn log_tool_contract_failure(report: &ToolContractValidationReport) {
        if let Ok(encoded) = serde_json::to_string(report) {
            eprintln!("sieve-runtime contract validation failure: {encoded}");
        } else {
            eprintln!("sieve-runtime contract validation failure");
        }
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
                    .await?
                    .action;
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

    async fn execute_mainline(
        &self,
        run_id: RunId,
        cwd: String,
        script: String,
        command_segments: Vec<CommandSegment>,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let report = self
            .mainline
            .run(MainlineRunRequest {
                run_id,
                cwd,
                script,
                command_segments,
            })
            .await?;
        Ok(RuntimeDisposition::ExecuteMainline(report))
    }

    async fn approve_tool_call(
        &self,
        run_id: RunId,
        segment: CommandSegment,
        blocked_rule_id: &str,
        reason: &str,
    ) -> Result<ApprovalResolution, RuntimeError> {
        self.request_approval(
            run_id,
            vec![segment],
            Vec::new(),
            blocked_rule_id.to_string(),
            reason.to_string(),
        )
        .await
    }

    async fn approve_explicit_tool_call(
        &self,
        run_id: RunId,
        segment: CommandSegment,
        fallback_blocked_rule_id: &str,
        fallback_reason: &str,
    ) -> Result<Option<ApprovalResolution>, RuntimeError> {
        let decision = self.evaluate_explicit_tool_policy(&run_id, &segment)?;
        match decision.kind {
            PolicyDecisionKind::Deny => Ok(None),
            PolicyDecisionKind::Allow => {
                let resolution = self
                    .approve_tool_call(run_id, segment, fallback_blocked_rule_id, fallback_reason)
                    .await?;
                Ok(Some(resolution))
            }
            PolicyDecisionKind::DenyWithApproval => {
                let blocked_rule_id = decision
                    .blocked_rule_id
                    .unwrap_or_else(|| fallback_blocked_rule_id.to_string());
                let resolution = self
                    .request_approval(
                        run_id,
                        vec![segment],
                        Vec::new(),
                        blocked_rule_id,
                        decision.reason,
                    )
                    .await?;
                Ok(Some(resolution))
            }
        }
    }

    fn evaluate_explicit_tool_policy(
        &self,
        run_id: &RunId,
        segment: &CommandSegment,
    ) -> Result<PolicyDecision, RuntimeError> {
        let runtime_context = self.runtime_policy_context_for_control(BTreeSet::new(), None)?;
        let input = PrecheckInput {
            run_id: run_id.clone(),
            cwd: ".".to_string(),
            command_segments: vec![segment.clone()],
            knowledge: CommandKnowledge::Known,
            summary: Some(CommandSummary {
                required_capabilities: Vec::new(),
                sink_checks: Vec::new(),
                unsupported_flags: Vec::new(),
            }),
            runtime_context,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        };
        Ok(self.policy.evaluate_precheck(&input))
    }

    async fn request_approval(
        &self,
        run_id: RunId,
        command_segments: Vec<CommandSegment>,
        inferred_capabilities: Vec<sieve_types::Capability>,
        blocked_rule_id: String,
        reason: String,
    ) -> Result<ApprovalResolution, RuntimeError> {
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
        Ok(ApprovalResolution {
            request_id: approval_resolved.request_id,
            action: approval_resolved.action,
        })
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

struct ApprovalResolution {
    request_id: ApprovalRequestId,
    action: ApprovalAction,
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
    use serde_json::{json, Value};
    use sieve_command_summaries::DefaultCommandSummarizer;
    use sieve_llm::LlmError;
    use sieve_policy::TomlPolicyEngine;
    use sieve_shell::{BasicShellAnalyzer, ShellAnalysis};
    use sieve_tool_contracts::TOOL_CONTRACTS_VERSION;
    use sieve_types::{
        Action, Capability, CommandKnowledge, CommandSummary, Integrity, LlmModelConfig,
        LlmProvider, PlannerToolCall, PlannerTurnInput, PlannerTurnOutput, PolicyDecision,
        Resource, SinkCheck, SinkKey, Source, ValueLabel, ValueRef,
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

    struct CapturingPolicy {
        decision: PolicyDecision,
        last_input: StdMutex<Option<PrecheckInput>>,
    }

    impl CapturingPolicy {
        fn new(decision: PolicyDecision) -> Self {
            Self {
                decision,
                last_input: StdMutex::new(None),
            }
        }

        fn captured_input(&self) -> PrecheckInput {
            self.last_input
                .lock()
                .expect("policy lock")
                .clone()
                .expect("captured precheck input")
        }
    }

    impl PolicyEngine for CapturingPolicy {
        fn evaluate_precheck(&self, input: &PrecheckInput) -> PolicyDecision {
            *self.last_input.lock().expect("policy lock") = Some(input.clone());
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

    struct StubMainline;

    #[async_trait]
    impl MainlineRunner for StubMainline {
        async fn run(
            &self,
            request: MainlineRunRequest,
        ) -> Result<MainlineRunReport, MainlineRunError> {
            Ok(MainlineRunReport {
                run_id: request.run_id,
                exit_code: Some(0),
            })
        }
    }

    struct CapturingMainline {
        exit_code: Option<i32>,
        requests: StdMutex<Vec<MainlineRunRequest>>,
    }

    impl CapturingMainline {
        fn new(exit_code: Option<i32>) -> Self {
            Self {
                exit_code,
                requests: StdMutex::new(Vec::new()),
            }
        }

        fn requests(&self) -> Vec<MainlineRunRequest> {
            self.requests.lock().expect("mainline lock").clone()
        }
    }

    #[async_trait]
    impl MainlineRunner for CapturingMainline {
        async fn run(
            &self,
            request: MainlineRunRequest,
        ) -> Result<MainlineRunReport, MainlineRunError> {
            self.requests
                .lock()
                .map_err(|_| MainlineRunError::Exec("mainline lock poisoned".to_string()))?
                .push(request.clone());
            Ok(MainlineRunReport {
                run_id: request.run_id,
                exit_code: self.exit_code,
            })
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

    struct CapturingPlanner {
        config: LlmModelConfig,
        output: PlannerTurnOutput,
        last_input: StdMutex<Option<PlannerTurnInput>>,
    }

    impl CapturingPlanner {
        fn new(output: PlannerTurnOutput) -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "gpt-test".to_string(),
                    api_base: None,
                },
                output,
                last_input: StdMutex::new(None),
            }
        }

        fn captured_input(&self) -> PlannerTurnInput {
            self.last_input
                .lock()
                .expect("planner lock")
                .clone()
                .expect("captured planner input")
        }
    }

    #[async_trait]
    impl PlannerModel for CapturingPlanner {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
            *self.last_input.lock().expect("planner lock") = Some(input);
            Ok(self.output.clone())
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
            capacity_type: sieve_types::CapacityType::Enum,
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
            mainline: Arc::new(StubMainline),
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

    fn mk_runtime_with_real_summary_and_policy(
        policy_toml: &str,
    ) -> (
        Arc<RuntimeOrchestrator>,
        Arc<InProcessApprovalBus>,
        Arc<VecEventLog>,
    ) {
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(VecEventLog::default());
        let policy = TomlPolicyEngine::from_toml_str(policy_toml).expect("policy parse");
        let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
            shell: Arc::new(BasicShellAnalyzer),
            summaries: Arc::new(DefaultCommandSummarizer),
            policy: Arc::new(policy),
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
            mainline: Arc::new(StubMainline),
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

    fn mk_runtime_with_capturing_planner(
        planner_output: PlannerTurnOutput,
        shell_knowledge: CommandKnowledge,
        segments: Vec<CommandSegment>,
        summary_knowledge: CommandKnowledge,
        policy_kind: PolicyDecisionKind,
    ) -> (
        Arc<RuntimeOrchestrator>,
        Arc<CapturingPlanner>,
        Arc<InProcessApprovalBus>,
        Arc<VecEventLog>,
    ) {
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(VecEventLog::default());
        let planner = Arc::new(CapturingPlanner::new(planner_output));
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
            mainline: Arc::new(StubMainline),
            planner: planner.clone(),
            approval_bus: approval_bus.clone(),
            event_log: event_log.clone(),
            clock: Arc::new(DeterministicClock::new(1000)),
        }));
        (runtime, planner, approval_bus, event_log)
    }

    fn mk_runtime_with_capturing_mainline(
        shell_knowledge: CommandKnowledge,
        segments: Vec<CommandSegment>,
        summary_knowledge: CommandKnowledge,
        policy_kind: PolicyDecisionKind,
        exit_code: Option<i32>,
    ) -> (
        Arc<RuntimeOrchestrator>,
        Arc<CapturingMainline>,
        Arc<InProcessApprovalBus>,
        Arc<VecEventLog>,
    ) {
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(VecEventLog::default());
        let mainline = Arc::new(CapturingMainline::new(exit_code));
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
            mainline: mainline.clone(),
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
        (runtime, mainline, approval_bus, event_log)
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

    async fn wait_for_approval_count(
        bus: &InProcessApprovalBus,
        count: usize,
    ) -> Vec<ApprovalRequestedEvent> {
        for _ in 0..20 {
            let published = bus.published_events().expect("published events");
            if published.len() >= count {
                return published;
            }
            sleep(Duration::from_millis(5)).await;
        }
        panic!("approval count not reached in time");
    }

    #[tokio::test]
    async fn orchestrate_shell_passes_runtime_context_to_policy() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string(), "ok".to_string()],
            operator_before: None,
        }];
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(VecEventLog::default());
        let policy = Arc::new(CapturingPolicy::new(PolicyDecision {
            kind: PolicyDecisionKind::Allow,
            reason: "allow".to_string(),
            blocked_rule_id: None,
        }));

        let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
            shell: Arc::new(StubShell {
                analysis: ShellAnalysis {
                    knowledge: CommandKnowledge::Known,
                    segments,
                    unsupported_constructs: Vec::new(),
                },
            }),
            summaries: Arc::new(StubSummaries {
                outcome: SummaryOutcome {
                    knowledge: CommandKnowledge::Known,
                    summary: Some(stub_summary()),
                    reason: None,
                },
            }),
            policy: policy.clone(),
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
            mainline: Arc::new(StubMainline),
            planner: Arc::new(StubPlanner {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "gpt-test".to_string(),
                    api_base: None,
                },
            }),
            approval_bus,
            event_log,
            clock: Arc::new(DeterministicClock::new(1000)),
        }));

        runtime
            .upsert_value_label(
                ValueRef("v_control".to_string()),
                label_with_sinks(Integrity::Untrusted, &[]),
            )
            .expect("insert control value label");
        runtime
            .upsert_value_label(
                ValueRef("v_payload".to_string()),
                label_with_sinks(Integrity::Trusted, &["https://example.com/path"]),
            )
            .expect("insert payload value label");

        let mut control_refs = BTreeSet::new();
        control_refs.insert(ValueRef("v_control".to_string()));
        let disposition = runtime
            .orchestrate_shell(ShellRunRequest {
                run_id: RunId("run-1".to_string()),
                cwd: "/tmp".to_string(),
                script: "echo ok".to_string(),
                control_value_refs: control_refs,
                control_endorsed_by: Some(ApprovalRequestId("approval-42".to_string())),
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect("runtime ok");
        match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run-1".to_string()));
                assert_eq!(report.exit_code, Some(0));
            }
            other => panic!("expected mainline execution, got {other:?}"),
        }

        let captured = policy.captured_input();
        assert_eq!(
            captured.runtime_context.control.integrity,
            Integrity::Untrusted
        );
        assert_eq!(
            captured.runtime_context.control.endorsed_by,
            Some(ApprovalRequestId("approval-42".to_string()))
        );
        assert!(captured
            .runtime_context
            .control
            .value_refs
            .contains(&ValueRef("v_control".to_string())));
        let sinks = captured
            .runtime_context
            .sink_permissions
            .allowed_sinks_by_value
            .get(&ValueRef("v_payload".to_string()))
            .expect("payload sink permissions");
        assert!(sinks.contains(&SinkKey("https://example.com/path".to_string())));
    }

    #[tokio::test]
    async fn orchestrate_shell_executes_mainline_with_segment_report() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string(), "ok".to_string()],
            operator_before: None,
        }];
        let (runtime, mainline, _approval_bus, _event_log) = mk_runtime_with_capturing_mainline(
            CommandKnowledge::Known,
            segments.clone(),
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
            Some(7),
        );

        let disposition = runtime
            .orchestrate_shell(ShellRunRequest {
                run_id: RunId("run-mainline".to_string()),
                cwd: "/tmp".to_string(),
                script: "echo ok".to_string(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect("runtime ok");

        match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run-mainline".to_string()));
                assert_eq!(report.exit_code, Some(7));
            }
            other => panic!("expected mainline execution, got {other:?}"),
        }

        let requests = mainline.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.run_id, RunId("run-mainline".to_string()));
        assert_eq!(request.cwd, "/tmp");
        assert_eq!(request.script, "echo ok");
        assert_eq!(request.command_segments, segments);
    }

    #[tokio::test]
    async fn orchestrate_planner_turn_executes_bash_through_policy_and_approval() {
        let mut args = BTreeMap::new();
        args.insert("cmd".to_string(), json!("rm -rf tmp"));
        let planner_output = PlannerTurnOutput {
            thoughts: Some("run approved command".to_string()),
            tool_calls: vec![PlannerToolCall {
                tool_name: "bash".to_string(),
                args,
            }],
        };
        let segments = vec![CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
            operator_before: None,
        }];
        let (runtime, planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
            planner_output,
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::DenyWithApproval,
        );

        let previous_events = vec![RuntimeEvent::ApprovalResolved(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-prev".to_string()),
            run_id: RunId("run-prev".to_string()),
            action: ApprovalAction::Deny,
            created_at_ms: 900,
        })];

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_planner_turn(PlannerRunRequest {
                        run_id: RunId("run-1".to_string()),
                        cwd: "/tmp".to_string(),
                        user_message: "delete tmp".to_string(),
                        allowed_tools: vec!["bash".to_string()],
                        previous_events,
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.blocked_rule_id, "rule-1");
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id.clone(),
                run_id: requested.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 1001,
            })
            .expect("resolve approval");

        let output = runtime_task
            .await
            .expect("task join")
            .expect("runtime planner turn");

        assert_eq!(output.thoughts, Some("run approved command".to_string()));
        assert_eq!(output.tool_results.len(), 1);
        match &output.tool_results[0] {
            PlannerToolResult::Bash {
                command,
                disposition,
            } => {
                assert_eq!(command, "rm -rf tmp");
                match disposition {
                    RuntimeDisposition::ExecuteMainline(report) => {
                        assert_eq!(report.run_id, RunId("run-1".to_string()));
                        assert_eq!(report.exit_code, Some(0));
                    }
                    other => panic!("expected mainline execution, got {other:?}"),
                }
            }
            other => panic!("expected bash result, got {other:?}"),
        }

        let planner_input = planner.captured_input();
        assert_eq!(planner_input.run_id, RunId("run-1".to_string()));
        assert_eq!(planner_input.user_message, "delete tmp");
        assert_eq!(planner_input.allowed_tools, vec!["bash".to_string()]);
        assert_eq!(planner_input.previous_events.len(), 1);
    }

    #[tokio::test]
    async fn orchestrate_planner_turn_runs_unknown_bash_in_quarantine_when_accepted() {
        let mut args = BTreeMap::new();
        args.insert("cmd".to_string(), json!("custom-cmd --flag"));
        let planner_output = PlannerTurnOutput {
            thoughts: None,
            tool_calls: vec![PlannerToolCall {
                tool_name: "bash".to_string(),
                args,
            }],
        };
        let segments = vec![CommandSegment {
            argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
            operator_before: None,
        }];
        let (runtime, _planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
            planner_output,
            CommandKnowledge::Unknown,
            segments,
            CommandKnowledge::Unknown,
            PolicyDecisionKind::Allow,
        );

        let output = runtime
            .orchestrate_planner_turn(PlannerRunRequest {
                run_id: RunId("run-1".to_string()),
                cwd: "/tmp".to_string(),
                user_message: "run custom command".to_string(),
                allowed_tools: vec!["bash".to_string()],
                previous_events: Vec::new(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Accept,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect("runtime planner turn");

        assert_eq!(output.tool_results.len(), 1);
        match &output.tool_results[0] {
            PlannerToolResult::Bash { disposition, .. } => {
                assert!(matches!(
                    disposition,
                    RuntimeDisposition::ExecuteQuarantine(_)
                ));
            }
            other => panic!("expected bash result, got {other:?}"),
        }
        assert!(approval_bus
            .published_events()
            .expect("published events")
            .is_empty());
    }

    #[tokio::test]
    async fn orchestrate_planner_turn_rejects_invalid_tool_args_with_contract_report() {
        let mut args = BTreeMap::new();
        args.insert("cmd".to_string(), json!(""));
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
        let (runtime, _planner, _approval_bus, _event_log) = mk_runtime_with_capturing_planner(
            planner_output,
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );

        let err = runtime
            .orchestrate_planner_turn(PlannerRunRequest {
                run_id: RunId("run-1".to_string()),
                cwd: "/tmp".to_string(),
                user_message: "run".to_string(),
                allowed_tools: vec!["bash".to_string()],
                previous_events: Vec::new(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect_err("invalid tool args should fail");

        match err {
            RuntimeError::ToolContract { report } => {
                assert_eq!(report.contract_version, TOOL_CONTRACTS_VERSION);
                assert_eq!(report.errors.len(), 1);
                let validation = &report.errors[0];
                assert_eq!(validation.tool_call_index, 0);
                assert_eq!(validation.tool_name, "bash");
                assert_eq!(validation.argument_path, "/cmd");
            }
            other => panic!("expected tool contract error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn orchestrate_planner_turn_rejects_disallowed_tool_before_dispatch() {
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
        let (runtime, planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
            planner_output,
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );

        let err = runtime
            .orchestrate_planner_turn(PlannerRunRequest {
                run_id: RunId("run-1".to_string()),
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
            .expect_err("disallowed tool should fail");

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
            other => panic!("expected disallowed tool error, got {other:?}"),
        }

        assert!(approval_bus
            .published_events()
            .expect("published events")
            .is_empty());
        let planner_input = planner.captured_input();
        assert_eq!(planner_input.allowed_tools, vec!["endorse".to_string()]);
    }

    #[tokio::test]
    async fn orchestrate_planner_turn_executes_endorse_with_approval() {
        let mut args = BTreeMap::new();
        args.insert("value_ref".to_string(), json!("v_control"));
        args.insert("target_integrity".to_string(), json!("trusted"));
        let planner_output = PlannerTurnOutput {
            thoughts: None,
            tool_calls: vec![PlannerToolCall {
                tool_name: "endorse".to_string(),
                args,
            }],
        };
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string(), "ok".to_string()],
            operator_before: None,
        }];
        let (runtime, _planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
            planner_output,
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );
        runtime
            .upsert_value_label(
                ValueRef("v_control".to_string()),
                label_with_sinks(Integrity::Untrusted, &[]),
            )
            .expect("seed value state");

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_planner_turn(PlannerRunRequest {
                        run_id: RunId("run-1".to_string()),
                        cwd: "/tmp".to_string(),
                        user_message: "endorse control".to_string(),
                        allowed_tools: vec!["endorse".to_string()],
                        previous_events: Vec::new(),
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.command_segments[0].argv[0], "endorse");
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id.clone(),
                run_id: requested.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 1001,
            })
            .expect("resolve approval");

        let output = runtime_task
            .await
            .expect("task join")
            .expect("runtime planner turn");
        assert_eq!(output.tool_results.len(), 1);
        match &output.tool_results[0] {
            PlannerToolResult::Endorse {
                request,
                transition: Some(transition),
            } => {
                assert_eq!(request.value_ref, ValueRef("v_control".to_string()));
                assert_eq!(transition.to_integrity, Integrity::Trusted);
                assert_eq!(transition.approved_by, Some(requested.request_id));
            }
            other => panic!("expected endorse transition, got {other:?}"),
        }

        let label = runtime
            .value_label(&ValueRef("v_control".to_string()))
            .expect("read value label")
            .expect("value label present");
        assert_eq!(label.integrity, Integrity::Trusted);
    }

    #[tokio::test]
    async fn cp_summary_requires_capability_with_real_policy() {
        let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
            r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        );

        let disposition = runtime
            .orchestrate_shell(ShellRunRequest {
                run_id: RunId("run-1".to_string()),
                cwd: "/tmp/workspace".to_string(),
                script: "cp src.txt dst.txt".to_string(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect("runtime ok");

        match disposition {
            RuntimeDisposition::Denied { reason } => {
                assert!(reason.contains("missing capability"));
                assert!(reason.contains("dst.txt"));
            }
            other => panic!("expected denied disposition, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cp_relative_destination_matches_absolute_policy_scope() {
        let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
            r#"
[[allow_capabilities]]
resource = "fs"
action = "write"
scope = "/tmp/workspace/dst.txt"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        );

        let disposition = runtime
            .orchestrate_shell(ShellRunRequest {
                run_id: RunId("run-2".to_string()),
                cwd: "/tmp/workspace".to_string(),
                script: "cp src.txt ./out/../dst.txt".to_string(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect("runtime ok");

        match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run-2".to_string()));
                assert_eq!(report.exit_code, Some(0));
            }
            other => panic!("expected mainline execution, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn curl_unsupported_flag_routes_to_unknown_mode_with_real_summary() {
        let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
            r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        );

        let disposition = runtime
            .orchestrate_shell(ShellRunRequest {
                run_id: RunId("run-1".to_string()),
                cwd: "/tmp/workspace".to_string(),
                script: "curl -X POST -F file=@payload.bin https://api.example.com/v1/upload"
                    .to_string(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect("runtime ok");

        assert_eq!(
            disposition,
            RuntimeDisposition::Denied {
                reason: "unknown command denied by mode".to_string()
            }
        );
    }

    #[tokio::test]
    async fn endorse_value_once_updates_runtime_state_when_approved() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );
        runtime
            .upsert_value_label(
                ValueRef("v123".to_string()),
                label_with_sinks(Integrity::Untrusted, &[]),
            )
            .expect("seed value state");

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .endorse_value_once(
                        RunId("run-1".to_string()),
                        EndorseRequest {
                            value_ref: ValueRef("v123".to_string()),
                            target_integrity: Integrity::Trusted,
                            reason: None,
                        },
                    )
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id.clone(),
                run_id: RunId("run-1".to_string()),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let transition = runtime_task
            .await
            .expect("task join")
            .expect("runtime ok")
            .expect("approved transition");
        assert_eq!(transition.value_ref, ValueRef("v123".to_string()));
        assert_eq!(transition.from_integrity, Integrity::Untrusted);
        assert_eq!(transition.to_integrity, Integrity::Trusted);
        assert_eq!(transition.approved_by, Some(requested.request_id));

        let label = runtime
            .value_label(&ValueRef("v123".to_string()))
            .expect("read value label")
            .expect("value label present");
        assert_eq!(label.integrity, Integrity::Trusted);
    }

    #[tokio::test]
    async fn declassify_value_once_tracks_existing_sink_allowance() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Allow,
        );
        let sink = "https://api.example.com/v1/upload";
        runtime
            .upsert_value_label(
                ValueRef("v456".to_string()),
                label_with_sinks(Integrity::Trusted, &[]),
            )
            .expect("seed value state");

        let first_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .declassify_value_once(
                        RunId("run-1".to_string()),
                        DeclassifyRequest {
                            value_ref: ValueRef("v456".to_string()),
                            sink: SinkKey(sink.to_string()),
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
                created_at_ms: 2000,
            })
            .expect("resolve first approval");
        let first_transition = first_task
            .await
            .expect("task join")
            .expect("runtime ok")
            .expect("approved transition");
        assert!(!first_transition.sink_was_already_allowed);

        let second_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .declassify_value_once(
                        RunId("run-2".to_string()),
                        DeclassifyRequest {
                            value_ref: ValueRef("v456".to_string()),
                            sink: SinkKey(sink.to_string()),
                            reason: None,
                        },
                    )
                    .await
            })
        };
        let second_requested = wait_for_approval_count(&approval_bus, 2).await[1].clone();
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: second_requested.request_id.clone(),
                run_id: second_requested.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 2001,
            })
            .expect("resolve second approval");
        let second_transition = second_task
            .await
            .expect("task join")
            .expect("runtime ok")
            .expect("approved transition");
        assert!(second_transition.sink_was_already_allowed);

        let label = runtime
            .value_label(&ValueRef("v456".to_string()))
            .expect("read value label")
            .expect("value label present");
        assert!(label.allowed_sinks.contains(&SinkKey(sink.to_string())));
    }

    #[tokio::test]
    async fn endorse_value_once_policy_deny_skips_approval_and_transition() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Deny,
        );
        runtime
            .upsert_value_label(
                ValueRef("v123".to_string()),
                label_with_sinks(Integrity::Untrusted, &[]),
            )
            .expect("seed value state");

        let transition = runtime
            .endorse_value_once(
                RunId("run-1".to_string()),
                EndorseRequest {
                    value_ref: ValueRef("v123".to_string()),
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
        let label = runtime
            .value_label(&ValueRef("v123".to_string()))
            .expect("read value label")
            .expect("value label present");
        assert_eq!(label.integrity, Integrity::Untrusted);
    }

    #[tokio::test]
    async fn endorse_value_once_policy_deny_with_approval_uses_policy_metadata() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::DenyWithApproval,
        );
        runtime
            .upsert_value_label(
                ValueRef("v123".to_string()),
                label_with_sinks(Integrity::Untrusted, &[]),
            )
            .expect("seed value state");

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .endorse_value_once(
                        RunId("run-1".to_string()),
                        EndorseRequest {
                            value_ref: ValueRef("v123".to_string()),
                            target_integrity: Integrity::Trusted,
                            reason: None,
                        },
                    )
                    .await
            })
        };

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.blocked_rule_id, "rule-1");
        assert_eq!(requested.reason, "policy verdict");
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id.clone(),
                run_id: requested.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 2000,
            })
            .expect("resolve approval");

        let transition = runtime_task
            .await
            .expect("task join")
            .expect("runtime ok")
            .expect("approved transition");
        assert_eq!(transition.value_ref, ValueRef("v123".to_string()));
        assert_eq!(transition.approved_by, Some(requested.request_id));
    }

    #[tokio::test]
    async fn declassify_value_once_policy_deny_skips_approval_and_transition() {
        let segments = vec![CommandSegment {
            argv: vec!["echo".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::Deny,
        );
        let sink = SinkKey("https://api.example.com/v1/upload".to_string());
        runtime
            .upsert_value_label(
                ValueRef("v456".to_string()),
                label_with_sinks(Integrity::Trusted, &[]),
            )
            .expect("seed value state");

        let transition = runtime
            .declassify_value_once(
                RunId("run-1".to_string()),
                DeclassifyRequest {
                    value_ref: ValueRef("v456".to_string()),
                    sink: sink.clone(),
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
        let label = runtime
            .value_label(&ValueRef("v456".to_string()))
            .expect("read value label")
            .expect("value label present");
        assert!(!label.allowed_sinks.contains(&sink));
    }

    #[test]
    fn approval_requested_event_schema_shape_stable() {
        let event = ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-1".to_string()),
            run_id: RunId("run-1".to_string()),
            command_segments: vec![CommandSegment {
                argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
                operator_before: None,
            }],
            inferred_capabilities: vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp".to_string(),
            }],
            blocked_rule_id: "rule-1".to_string(),
            reason: "requires approval".to_string(),
            created_at_ms: 1000,
        };
        let as_json = serde_json::to_value(&event).expect("serialize");
        let obj = as_json.as_object().expect("event object");
        for key in [
            "schema_version",
            "request_id",
            "run_id",
            "command_segments",
            "inferred_capabilities",
            "blocked_rule_id",
            "reason",
            "created_at_ms",
        ] {
            assert!(obj.contains_key(key), "missing key: {key}");
        }
        assert_eq!(obj.len(), 8);
        assert_eq!(obj.get("schema_version"), Some(&Value::from(1)));
        assert_eq!(obj.get("request_id"), Some(&Value::from("approval-1")));
        assert_eq!(obj.get("run_id"), Some(&Value::from("run-1")));
    }

    #[test]
    fn approval_resolved_event_schema_shape_stable() {
        let event = ApprovalResolvedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-1".to_string()),
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1001,
        };
        let as_json = serde_json::to_value(&event).expect("serialize");
        let obj = as_json.as_object().expect("event object");
        for key in [
            "schema_version",
            "request_id",
            "run_id",
            "action",
            "created_at_ms",
        ] {
            assert!(obj.contains_key(key), "missing key: {key}");
        }
        assert_eq!(obj.len(), 5);
        assert_eq!(obj.get("action"), Some(&Value::from("approve_once")));
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
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
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
        match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run-1".to_string()));
                assert_eq!(report.exit_code, Some(0));
            }
            other => panic!("expected mainline execution, got {other:?}"),
        }

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
    async fn approval_bus_concurrent_requests_do_not_cross_resolve() {
        let segments = vec![CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, _event_log) = mk_runtime(
            CommandKnowledge::Known,
            segments,
            CommandKnowledge::Known,
            PolicyDecisionKind::DenyWithApproval,
        );

        let task_1 = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run-1".to_string()),
                        cwd: "/tmp".to_string(),
                        script: "rm -rf tmp".to_string(),
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };
        let task_2 = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run-2".to_string()),
                        cwd: "/tmp".to_string(),
                        script: "rm -rf tmp".to_string(),
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            })
        };

        let published = wait_for_approval_count(&approval_bus, 2).await;
        assert_eq!(published.len(), 2);
        let req_1 = published[0].clone();
        let req_2 = published[1].clone();
        assert_ne!(req_1.request_id, req_2.request_id);

        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: req_2.request_id.clone(),
                run_id: req_2.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 3000,
            })
            .expect("resolve second");
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: req_1.request_id.clone(),
                run_id: req_1.run_id.clone(),
                action: ApprovalAction::ApproveOnce,
                created_at_ms: 3001,
            })
            .expect("resolve first");

        let out_1 = task_1.await.expect("join 1").expect("runtime 1");
        let out_2 = task_2.await.expect("join 2").expect("runtime 2");
        match out_1 {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run-1".to_string()));
                assert_eq!(report.exit_code, Some(0));
            }
            other => panic!("expected mainline execution, got {other:?}"),
        }
        match out_2 {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run-2".to_string()));
                assert_eq!(report.exit_code, Some(0));
            }
            other => panic!("expected mainline execution, got {other:?}"),
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
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
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
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
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
    async fn uncertain_ask_requires_approval_before_quarantine() {
        let segments = vec![CommandSegment {
            argv: vec!["weird-shell-construct".to_string()],
            operator_before: None,
        }];
        let (runtime, approval_bus, event_log) = mk_runtime(
            CommandKnowledge::Uncertain,
            segments,
            CommandKnowledge::Uncertain,
            PolicyDecisionKind::Allow,
        );

        let runtime_task = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run-1".to_string()),
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

        let requested = wait_for_approval(&approval_bus).await;
        assert_eq!(requested.blocked_rule_id, "uncertain_command_mode");
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
    async fn endorse_request_deny_path_records_resolution() {
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
        match &events[1] {
            RuntimeEvent::ApprovalResolved(e) => assert_eq!(e.action, ApprovalAction::Deny),
            _ => panic!("expected approval resolved"),
        }
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
    async fn declassify_request_deny_path_records_resolution() {
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
        match &events[1] {
            RuntimeEvent::ApprovalResolved(e) => assert_eq!(e.action, ApprovalAction::Deny),
            _ => panic!("expected approval resolved"),
        }
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
}
