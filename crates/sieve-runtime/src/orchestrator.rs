use crate::approval_allowance::ApprovalAllowanceKey;
use crate::approval_bus::{ApprovalBus, ApprovalBusError};
use crate::automation::AutomationTool;
use crate::browser_sessions::BrowserSessionState;
use crate::codex::CodexTool;
use crate::event_log::{EventLogError, RuntimeEventLog};
use crate::mainline::{MainlineRunError, MainlineRunner};
use crate::value_state::{RuntimeValueState, ValueStateError};
use sieve_command_summaries::CommandSummarizer;
use sieve_llm::{LlmError, PlannerModel};
use sieve_policy::PolicyEngine;
use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
use sieve_types::{
    ApprovalAction, ApprovalPromptKind, ApprovalRequestId, ApprovalRequestedEvent, Capability,
    CommandSegment, PlannerBrowserSession, PlannerCodexSession, RunId, RuntimeEvent,
    RuntimePolicyContext, ToolContractValidationReport, ValueLabel, ValueRef,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

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
    #[error("automation tool failed: {0}")]
    Automation(String),
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
    pub(crate) shell: Arc<dyn ShellAnalyzer>,
    pub(crate) summaries: Arc<dyn CommandSummarizer>,
    pub(crate) policy: Arc<dyn PolicyEngine>,
    pub(crate) quarantine: Arc<dyn QuarantineRunner>,
    pub(crate) mainline: Arc<dyn MainlineRunner>,
    pub(crate) planner: Arc<dyn PlannerModel>,
    pub(crate) automation: Option<Arc<dyn AutomationTool>>,
    pub(crate) codex: Option<Arc<dyn CodexTool>>,
    pub(crate) approval_bus: Arc<dyn ApprovalBus>,
    pub(crate) event_log: Arc<dyn RuntimeEventLog>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) next_request: AtomicU64,
    pub(crate) value_state: Mutex<RuntimeValueState>,
    pub(crate) persistent_approval_allowances: Mutex<BTreeSet<ApprovalAllowanceKey>>,
    pub(crate) browser_sessions: Mutex<BTreeMap<String, BrowserSessionState>>,
    pub(crate) bash_placeholder_values: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
}

pub struct RuntimeDeps {
    pub shell: Arc<dyn ShellAnalyzer>,
    pub summaries: Arc<dyn CommandSummarizer>,
    pub policy: Arc<dyn PolicyEngine>,
    pub quarantine: Arc<dyn QuarantineRunner>,
    pub mainline: Arc<dyn MainlineRunner>,
    pub planner: Arc<dyn PlannerModel>,
    pub automation: Option<Arc<dyn AutomationTool>>,
    pub codex: Option<Arc<dyn CodexTool>>,
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
            automation: deps.automation,
            codex: deps.codex,
            approval_bus: deps.approval_bus,
            event_log: deps.event_log,
            clock: deps.clock,
            next_request: AtomicU64::new(1),
            value_state: Mutex::new(RuntimeValueState::default()),
            persistent_approval_allowances: Mutex::new(BTreeSet::new()),
            browser_sessions: Mutex::new(BTreeMap::new()),
            bash_placeholder_values: Mutex::new(BTreeMap::new()),
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

    pub(crate) fn browser_sessions_snapshot(
        &self,
    ) -> Result<BTreeMap<String, BrowserSessionState>, RuntimeError> {
        let sessions = self
            .browser_sessions
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(sessions.clone())
    }

    pub fn planner_browser_sessions(&self) -> Result<Vec<PlannerBrowserSession>, RuntimeError> {
        let sessions = self
            .browser_sessions
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        Ok(sessions
            .iter()
            .map(|(session_name, state)| PlannerBrowserSession {
                session_name: session_name.clone(),
                current_origin: state.current_origin.clone(),
                current_url: state.current_url.clone(),
            })
            .collect())
    }

    pub fn has_automation_tool(&self) -> bool {
        self.automation.is_some()
    }

    pub fn set_bash_placeholder_values(
        &self,
        run_id: &RunId,
        placeholders: BTreeMap<String, String>,
    ) -> Result<(), RuntimeError> {
        let mut store = self
            .bash_placeholder_values
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        store.insert(run_id.0.clone(), placeholders);
        Ok(())
    }

    pub(crate) fn expand_bash_placeholders(
        &self,
        run_id: &RunId,
        script: &str,
    ) -> Result<String, RuntimeError> {
        let store = self
            .bash_placeholder_values
            .lock()
            .map_err(|_| ValueStateError::LockPoisoned)?;
        let mut expanded = script.to_string();
        if let Some(placeholders) = store.get(&run_id.0) {
            for (placeholder, value) in placeholders {
                expanded = expanded.replace(placeholder, value);
            }
        }
        Ok(expanded)
    }

    pub fn has_codex_tool(&self) -> bool {
        self.codex.is_some()
    }

    pub fn codex_tool(&self) -> Option<Arc<dyn CodexTool>> {
        self.codex.clone()
    }

    pub async fn planner_codex_sessions(&self) -> Result<Vec<PlannerCodexSession>, RuntimeError> {
        match &self.codex {
            Some(codex) => codex
                .planner_sessions()
                .await
                .map_err(RuntimeError::Automation),
            None => Ok(Vec::new()),
        }
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

    pub(crate) async fn request_approval(
        &self,
        run_id: RunId,
        command_segments: Vec<CommandSegment>,
        inferred_capabilities: Vec<sieve_types::Capability>,
        blocked_rule_id: String,
        reason: String,
    ) -> Result<ApprovalResolution, RuntimeError> {
        let request_id = self.new_request_id(&run_id);
        let approval_requested = ApprovalRequestedEvent {
            schema_version: 1,
            request_id: request_id.clone(),
            run_id,
            prompt_kind: ApprovalPromptKind::Command,
            title: None,
            command_segments,
            inferred_capabilities,
            blocked_rule_id,
            reason,
            preview: None,
            reply_to_session_id: None,
            allow_approve_always: true,
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

    pub(crate) async fn append_event(&self, event: RuntimeEvent) -> Result<(), RuntimeError> {
        self.event_log.append(event).await?;
        Ok(())
    }

    fn new_request_id(&self, run_id: &RunId) -> ApprovalRequestId {
        let next = self.next_request.fetch_add(1, Ordering::Relaxed);
        if let Some((session_id, _turn_suffix)) = run_id.0.rsplit_once("-t") {
            return ApprovalRequestId(format!("apr-{session_id}-{next}"));
        }
        ApprovalRequestId(format!("approval-{next}"))
    }
}

pub(crate) struct ApprovalResolution {
    pub(crate) request_id: ApprovalRequestId,
    pub(crate) action: ApprovalAction,
}
