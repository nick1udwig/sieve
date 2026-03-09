use crate::{
    ApprovalAction, ApprovalRequestId, Capability, CommandSegment, PolicyDecision, RunId,
    UnixMillis,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPromptKind {
    Command,
    FileChange,
}

const fn default_approval_prompt_kind() -> ApprovalPromptKind {
    ApprovalPromptKind::Command
}

const fn default_allow_approve_always() -> bool {
    true
}

/// Event emitted when runtime asks user for command approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestedEvent {
    pub schema_version: u16,
    pub request_id: ApprovalRequestId,
    pub run_id: RunId,
    #[serde(default = "default_approval_prompt_kind")]
    pub prompt_kind: ApprovalPromptKind,
    #[serde(default)]
    pub title: Option<String>,
    pub command_segments: Vec<CommandSegment>,
    pub inferred_capabilities: Vec<Capability>,
    pub blocked_rule_id: String,
    pub reason: String,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default = "default_allow_approve_always")]
    pub allow_approve_always: bool,
    pub created_at_ms: UnixMillis,
}

/// Event emitted when user resolves an approval prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResolvedEvent {
    pub schema_version: u16,
    pub request_id: ApprovalRequestId,
    pub run_id: RunId,
    pub action: ApprovalAction,
    pub created_at_ms: UnixMillis,
}

/// Event emitted after policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEvaluatedEvent {
    pub schema_version: u16,
    pub run_id: RunId,
    pub decision: PolicyDecision,
    pub inferred_capabilities: Vec<Capability>,
    pub trace_path: Option<String>,
    pub created_at_ms: UnixMillis,
}

/// Request to execute a composed command in quarantine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub command_segments: Vec<CommandSegment>,
}

/// Quarantine execution report payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineReport {
    pub run_id: RunId,
    pub trace_path: String,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub attempted_capabilities: Vec<Capability>,
    pub exit_code: Option<i32>,
}

/// Event emitted after quarantine run completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineCompletedEvent {
    pub schema_version: u16,
    pub run_id: RunId,
    pub report: QuarantineReport,
    pub created_at_ms: UnixMillis,
}

/// Event emitted when assistant text is ready for user delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMessageEvent {
    pub schema_version: u16,
    pub run_id: RunId,
    pub message: String,
    pub created_at_ms: UnixMillis,
}

/// Union of runtime audit events written to JSONL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RuntimeEvent {
    ApprovalRequested(ApprovalRequestedEvent),
    ApprovalResolved(ApprovalResolvedEvent),
    PolicyEvaluated(PolicyEvaluatedEvent),
    QuarantineCompleted(QuarantineCompletedEvent),
    AssistantMessage(AssistantMessageEvent),
}
