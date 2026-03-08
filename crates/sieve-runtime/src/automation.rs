use async_trait::async_trait;
use sieve_types::{AutomationRequest, TrustedToolEffect};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationToolResult {
    pub message: String,
    pub effect: Option<TrustedToolEffect>,
}

#[async_trait]
pub trait AutomationTool: Send + Sync {
    async fn handle_request(
        &self,
        request: AutomationRequest,
    ) -> Result<AutomationToolResult, String>;
}
