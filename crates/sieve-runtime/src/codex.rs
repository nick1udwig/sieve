use async_trait::async_trait;
use sieve_types::{
    CodexExecRequest, CodexExecResult, CodexSessionRequest, CodexTurnResult, PlannerCodexSession,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecToolResult {
    pub result: CodexExecResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexSessionToolResult {
    pub result: CodexTurnResult,
}

#[async_trait]
pub trait CodexTool: Send + Sync {
    async fn exec(&self, request: CodexExecRequest) -> Result<CodexExecToolResult, String>;

    async fn run_task(
        &self,
        request: CodexSessionRequest,
    ) -> Result<CodexSessionToolResult, String>;

    async fn run_session(
        &self,
        request: CodexSessionRequest,
    ) -> Result<CodexSessionToolResult, String>;

    async fn steer_session(&self, _session_id: &str, _text: &str) -> Result<(), String> {
        Err("codex steer not supported".to_string())
    }

    async fn planner_sessions(&self) -> Result<Vec<PlannerCodexSession>, String>;
}
