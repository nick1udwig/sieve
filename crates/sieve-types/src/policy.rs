use serde::{Deserialize, Serialize};

/// High-level policy decision class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    Allow,
    DenyWithApproval,
    Deny,
}

/// Policy decision with human-readable reasoning metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub kind: PolicyDecisionKind,
    pub reason: String,
    pub blocked_rule_id: Option<String>,
}

/// User decision for an approval prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    ApproveOnce,
    ApproveAlways,
    Deny,
}
