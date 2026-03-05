#![forbid(unsafe_code)]

mod approval_allowance;
mod approval_bus;
mod event_log;
mod value_state;

use async_trait::async_trait;
use approval_allowance::{ApprovalAllowanceKey, UnknownOrUncertain};
use sieve_command_summaries::{CommandSummarizer, SummaryOutcome};
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::PolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
use sieve_tool_contracts::{validate_at_index, TypedCall, TOOL_CONTRACTS_VERSION};
use sieve_types::{
    ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, Capability,
    CommandKnowledge, CommandSegment, CommandSummary, DeclassifyRequest, DeclassifyStateTransition,
    EndorseRequest, EndorseStateTransition, PlannerGuidanceFrame, PlannerToolCall,
    PlannerTurnInput, PolicyDecision, PolicyDecisionKind, PolicyEvaluatedEvent, PrecheckInput,
    QuarantineCompletedEvent, QuarantineReport, QuarantineRunRequest, RunId,
    RuntimeEvent, RuntimePolicyContext, ToolContractValidationReport, UncertainMode, UnknownMode,
    ValueLabel, ValueRef,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::process::Command as TokioCommand;

pub use approval_bus::{ApprovalBus, ApprovalBusError, InProcessApprovalBus};
pub use event_log::{EventLogError, JsonlRuntimeEventLog, RuntimeEventLog};
use value_state::RuntimeValueState;
pub use value_state::ValueStateError;

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
        Ok(state.value_label(value_ref))
    }

    pub fn has_known_value_refs(&self) -> Result<bool, RuntimeError> {
        let state = self
            .value_state
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(state.has_any_labels())
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

struct ApprovalResolution {
    request_id: ApprovalRequestId,
    action: ApprovalAction,
}

#[cfg(test)]
mod tests;
