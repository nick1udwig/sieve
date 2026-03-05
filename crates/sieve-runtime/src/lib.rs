#![forbid(unsafe_code)]

mod approval_allowance;
mod approval_bus;
mod approval_tools;
mod event_log;
mod mainline;
mod planner_turn;
mod shell_gate;
mod value_state;

use approval_allowance::ApprovalAllowanceKey;
use sieve_command_summaries::CommandSummarizer;
#[cfg(test)]
use sieve_command_summaries::SummaryOutcome;
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::PolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
use sieve_types::{
    ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, Capability, CommandSegment, RunId,
    RuntimeEvent, RuntimePolicyContext, ToolContractValidationReport, ValueLabel, ValueRef,
};
#[cfg(test)]
use sieve_types::{
    DeclassifyRequest, EndorseRequest, PolicyDecisionKind, PrecheckInput, QuarantineReport,
    QuarantineRunRequest, UncertainMode, UnknownMode,
};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub use approval_bus::{ApprovalBus, ApprovalBusError, InProcessApprovalBus};
pub use event_log::{EventLogError, JsonlRuntimeEventLog, RuntimeEventLog};
pub use mainline::{
    BashMainlineRunner, MainlineArtifact, MainlineArtifactKind, MainlineRunError,
    MainlineRunReport, MainlineRunRequest, MainlineRunner,
};
pub use planner_turn::{PlannerRunRequest, PlannerRunResult, PlannerToolResult};
pub use shell_gate::{RuntimeDisposition, ShellRunRequest};
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
