use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexSandboxMode {
    ReadOnly,
    WorkspaceWrite,
}

impl CodexSandboxMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceWrite => "workspace_write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexExecRequest {
    pub instruction: String,
    pub sandbox: CodexSandboxMode,
    pub cwd: Option<String>,
    #[serde(default)]
    pub writable_roots: Vec<String>,
    #[serde(default)]
    pub local_images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexSessionRequest {
    pub session_id: Option<String>,
    pub instruction: String,
    pub sandbox: CodexSandboxMode,
    pub cwd: Option<String>,
    #[serde(default)]
    pub writable_roots: Vec<String>,
    #[serde(default)]
    pub local_images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexTurnStatus {
    Completed,
    NeedsFollowup,
    Failed,
}

impl CodexTurnStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NeedsFollowup => "needs_followup",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexTurnResult {
    pub session_id: Option<String>,
    pub session_name: String,
    pub status: CodexTurnStatus,
    pub summary: String,
    pub user_visible: Option<String>,
    pub turn_id: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerCodexSession {
    pub session_id: String,
    pub session_name: String,
    pub cwd: String,
    pub sandbox: CodexSandboxMode,
    pub updated_at_utc: String,
    pub status: String,
    pub task_summary: String,
    pub last_result_summary: Option<String>,
}
