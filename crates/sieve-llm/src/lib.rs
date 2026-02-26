#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_types::{
    LlmModelConfig, PlannerTurnInput, PlannerTurnOutput, QuarantineExtractInput,
    QuarantineExtractOutput,
};
use thiserror::Error;

mod config;
mod openai;
mod wire;

#[cfg(test)]
mod tests;

pub use config::LlmConfigs;
pub use openai::{OpenAiPlannerModel, OpenAiQuarantineModel};

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("llm backend error: {0}")]
    Backend(String),
    #[error("llm config error: {0}")]
    Config(String),
    #[error("llm transport error: {0}")]
    Transport(String),
    #[error("llm backend status {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("llm decode error: {0}")]
    Decode(String),
    #[error("planner boundary violation: {0}")]
    Boundary(String),
    #[error("llm retry exhausted: {0}")]
    RetryExhausted(String),
}

#[async_trait]
pub trait PlannerModel: Send + Sync {
    fn config(&self) -> &LlmModelConfig;

    async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError>;
}

#[async_trait]
pub trait QuarantineModel: Send + Sync {
    fn config(&self) -> &LlmModelConfig;

    async fn extract_typed(
        &self,
        input: QuarantineExtractInput,
    ) -> Result<QuarantineExtractOutput, LlmError>;
}
