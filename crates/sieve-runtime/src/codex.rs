use async_trait::async_trait;
use sieve_types::{CodexExecRequest, CodexSessionRequest, CodexTurnResult, PlannerCodexSession};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexToolResult {
    pub result: CodexTurnResult,
}

#[async_trait]
pub trait CodexTool: Send + Sync {
    async fn exec(&self, request: CodexExecRequest) -> Result<CodexToolResult, String>;

    async fn run_session(&self, request: CodexSessionRequest) -> Result<CodexToolResult, String>;

    async fn planner_sessions(&self) -> Result<Vec<PlannerCodexSession>, String>;
}
