#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, SummaryOutcome};
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::PolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
use sieve_tool_contracts::{validate_at_index, TypedCall, TOOL_CONTRACTS_VERSION};
use sieve_types::{
    Action, ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, ApprovalResolvedEvent,
    Capability, CommandKnowledge, CommandSegment, CommandSummary, ControlContext,
    DeclassifyRequest, DeclassifyStateTransition, EndorseRequest, EndorseStateTransition,
    Integrity, PlannerGuidanceFrame, PlannerToolCall, PlannerTurnInput, PolicyDecision,
    PolicyDecisionKind, PolicyEvaluatedEvent, PrecheckInput, QuarantineCompletedEvent,
    QuarantineReport, QuarantineRunRequest, Resource, RunId, RuntimeEvent, RuntimePolicyContext,
    SinkKey, SinkPermissionContext, ToolContractValidationReport, UncertainMode, UnknownMode,
    ValueLabel, ValueRef,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::process::Command as TokioCommand;
use tokio::sync::oneshot;
use url::Url;

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

    pub async fn append_json_value(&self, value: &serde_json::Value) -> Result<(), EventLogError> {
        let encoded =
            serde_json::to_string(value).map_err(|err| EventLogError::Append(err.to_string()))?;
        self.append_encoded_line(&encoded)
    }

    fn append_encoded_line(&self, encoded: &str) -> Result<(), EventLogError> {
        let _guard = self
            .writer_lock
            .lock()
            .map_err(|_| EventLogError::Append("event writer lock poisoned".to_string()))?;
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

#[async_trait]
impl RuntimeEventLog for JsonlRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        let encoded =
            serde_json::to_string(&event).map_err(|err| EventLogError::Append(err.to_string()))?;
        self.append_encoded_line(&encoded)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainlineArtifactKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineArtifact {
    pub ref_id: String,
    pub kind: MainlineArtifactKind,
    pub path: String,
    pub byte_count: u64,
    pub line_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineRunReport {
    pub run_id: RunId,
    pub exit_code: Option<i32>,
    pub artifacts: Vec<MainlineArtifact>,
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
        let status = TokioCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .status()
            .await
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: status.code(),
            artifacts: Vec::new(),
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
    pub allowed_net_connect_scopes: Vec<String>,
    pub previous_events: Vec<RuntimeEvent>,
    pub guidance: Option<PlannerGuidanceFrame>,
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
    persistent_approval_allowances: Mutex<BTreeSet<ApprovalAllowanceKey>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ApprovalAllowanceKey {
    resource: Resource,
    action: Action,
    scope: String,
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
            persistent_approval_allowances: Mutex::new(BTreeSet::new()),
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

    pub fn has_known_value_refs(&self) -> Result<bool, RuntimeError> {
        let state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(!state.labels_by_value.is_empty())
    }

    pub fn persistent_approval_allowances(&self) -> Result<Vec<Capability>, RuntimeError> {
        let allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(allowances
            .iter()
            .map(ApprovalAllowanceKey::as_capability)
            .collect())
    }

    pub fn restore_persistent_approval_allowances(
        &self,
        capabilities: &[Capability],
    ) -> Result<(), RuntimeError> {
        let mut allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        for capability in capabilities {
            allowances.insert(ApprovalAllowanceKey::for_capability(capability));
        }
        Ok(())
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
                allowed_net_connect_scopes: request.allowed_net_connect_scopes.clone(),
                previous_events: request.previous_events.clone(),
                guidance: request.guidance.clone(),
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
                    runtime_context.clone(),
                    shell.segments,
                    UnknownOrUncertain::Unknown,
                )
                .await;
        }

        if knowledge == CommandKnowledge::Uncertain {
            return self
                .handle_unknown_or_uncertain(
                    &request,
                    runtime_context.clone(),
                    shell.segments,
                    UnknownOrUncertain::Uncertain,
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
                if decision.blocked_rule_id.as_deref() == Some("missing-capability")
                    && self.capabilities_persistently_allowed(&inferred_capabilities)?
                {
                    return self
                        .execute_mainline(
                            request.run_id,
                            request.cwd,
                            request.script,
                            command_segments,
                        )
                        .await;
                }
                let blocked_rule_id = decision
                    .blocked_rule_id
                    .unwrap_or_else(|| "deny_with_approval".to_string());
                let resolution = self
                    .request_approval(
                        request.run_id.clone(),
                        command_segments.clone(),
                        inferred_capabilities.clone(),
                        blocked_rule_id,
                        decision.reason,
                    )
                    .await?;
                match resolution.action {
                    ApprovalAction::ApproveOnce | ApprovalAction::ApproveAlways => {
                        if resolution.action == ApprovalAction::ApproveAlways {
                            self.remember_persistent_approval_allowances(&inferred_capabilities)?;
                        }
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

    fn remember_persistent_approval_allowances(
        &self,
        capabilities: &[Capability],
    ) -> Result<(), RuntimeError> {
        if capabilities.is_empty() {
            return Ok(());
        }
        let mut allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        for capability in capabilities {
            allowances.insert(ApprovalAllowanceKey::for_capability(capability));
        }
        Ok(())
    }

    fn capabilities_persistently_allowed(
        &self,
        capabilities: &[Capability],
    ) -> Result<bool, RuntimeError> {
        if capabilities.is_empty() {
            return Ok(false);
        }
        let allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(capabilities
            .iter()
            .map(ApprovalAllowanceKey::for_capability)
            .all(|key| allowances.contains(&key)))
    }

    fn remember_unknown_or_uncertain_allowance(
        &self,
        kind: UnknownOrUncertain,
        command_segments: &[CommandSegment],
    ) -> Result<(), RuntimeError> {
        let mut allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        allowances.insert(ApprovalAllowanceKey::for_unknown_or_uncertain(
            kind,
            command_segments,
        ));
        Ok(())
    }

    fn unknown_or_uncertain_persistently_allowed(
        &self,
        kind: UnknownOrUncertain,
        command_segments: &[CommandSegment],
    ) -> Result<bool, RuntimeError> {
        let allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(
            allowances.contains(&ApprovalAllowanceKey::for_unknown_or_uncertain(
                kind,
                command_segments,
            )),
        )
    }

    async fn handle_unknown_or_uncertain(
        &self,
        request: &ShellRunRequest,
        runtime_context: RuntimePolicyContext,
        segments: Vec<CommandSegment>,
        kind: UnknownOrUncertain,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let precheck = PrecheckInput {
            run_id: request.run_id.clone(),
            cwd: request.cwd.clone(),
            command_segments: segments.clone(),
            knowledge: kind.to_knowledge(),
            summary: None,
            runtime_context,
            unknown_mode: request.unknown_mode,
            uncertain_mode: request.uncertain_mode,
        };
        let decision = self.policy.evaluate_precheck(&precheck);
        let policy_event = PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: request.run_id.clone(),
            decision: decision.clone(),
            inferred_capabilities: Vec::new(),
            trace_path: None,
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::PolicyEvaluated(policy_event))
            .await?;

        match decision.kind {
            PolicyDecisionKind::Deny => Ok(RuntimeDisposition::Denied {
                reason: decision.reason,
            }),
            PolicyDecisionKind::DenyWithApproval => {
                let blocked_rule_id = kind.to_blocked_rule_id().to_string();
                if self.unknown_or_uncertain_persistently_allowed(kind, &segments)? {
                    let report = self
                        .run_quarantine(request.run_id.clone(), request.cwd.clone(), segments)
                        .await?;
                    return Ok(RuntimeDisposition::ExecuteQuarantine(report));
                }
                let action = self
                    .request_approval(
                        request.run_id.clone(),
                        segments.clone(),
                        Vec::new(),
                        blocked_rule_id.clone(),
                        decision.reason,
                    )
                    .await?
                    .action;
                match action {
                    ApprovalAction::ApproveOnce | ApprovalAction::ApproveAlways => {
                        if action == ApprovalAction::ApproveAlways {
                            self.remember_unknown_or_uncertain_allowance(kind, &segments)?;
                        }
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
            PolicyDecisionKind::Allow => {
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

impl ApprovalAllowanceKey {
    fn for_capability(capability: &Capability) -> Self {
        Self {
            resource: capability.resource,
            action: capability.action,
            scope: canonical_approval_scope(capability),
        }
    }

    fn for_unknown_or_uncertain(
        kind: UnknownOrUncertain,
        command_segments: &[CommandSegment],
    ) -> Self {
        Self {
            resource: Resource::Proc,
            action: Action::Exec,
            scope: canonical_unknown_or_uncertain_scope(kind, command_segments),
        }
    }

    fn as_capability(&self) -> Capability {
        Capability {
            resource: self.resource,
            action: self.action,
            scope: self.scope.clone(),
        }
    }
}

fn canonical_approval_scope(capability: &Capability) -> String {
    match (capability.resource, capability.action) {
        (Resource::Net, Action::Connect) => canonical_net_origin_scope(&capability.scope)
            .unwrap_or_else(|| capability.scope.clone()),
        _ => capability.scope.clone(),
    }
}

fn canonical_net_origin_scope(scope: &str) -> Option<String> {
    let url = Url::parse(scope).ok()?;
    let host = url.host_str()?;
    let mut origin = format!("{}://{}", url.scheme(), host.to_ascii_lowercase());
    if let Some(port) = url.port() {
        let default_port = match url.scheme() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        if Some(port) != default_port {
            origin.push(':');
            origin.push_str(&port.to_string());
        }
    }
    Some(origin)
}

fn canonical_unknown_or_uncertain_scope(
    kind: UnknownOrUncertain,
    command_segments: &[CommandSegment],
) -> String {
    let encoded = serde_json::to_string(command_segments).unwrap_or_else(|_| {
        command_segments
            .iter()
            .map(|segment| segment.argv.join(" "))
            .collect::<Vec<_>>()
            .join(" && ")
    });
    format!("{}::{encoded}", kind.to_blocked_rule_id())
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

    fn to_knowledge(self) -> CommandKnowledge {
        match self {
            Self::Unknown => CommandKnowledge::Unknown,
            Self::Uncertain => CommandKnowledge::Uncertain,
        }
    }
}

#[cfg(test)]
mod tests;
