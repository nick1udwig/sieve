#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_types::{
    DeliveryContext, InteractionModality, LlmModelConfig, PlannerGuidanceInput,
    PlannerGuidanceOutput, PlannerTurnInput, PlannerTurnOutput, ResolvedPersonality, RunId,
};
use std::collections::BTreeSet;
use thiserror::Error;

mod config;
mod openai;
mod wire;

#[cfg(test)]
mod tests;

pub use config::LlmConfigs;
pub use openai::{
    OpenAiGuidanceModel, OpenAiPlannerModel, OpenAiResponseModel, OpenAiSummaryModel,
};

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
pub trait GuidanceModel: Send + Sync {
    fn config(&self) -> &LlmModelConfig;

    async fn classify_guidance(
        &self,
        input: PlannerGuidanceInput,
    ) -> Result<PlannerGuidanceOutput, LlmError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseRefMetadata {
    pub ref_id: String,
    pub kind: String,
    pub byte_count: u64,
    pub line_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseToolOutcome {
    pub tool_name: String,
    pub outcome: String,
    pub attempted_command: Option<String>,
    pub failure_reason: Option<String>,
    pub refs: Vec<ResponseRefMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseTurnInput {
    pub run_id: RunId,
    pub trusted_user_message: String,
    pub delivery_context: DeliveryContext,
    pub response_modality: InteractionModality,
    pub resolved_personality: ResolvedPersonality,
    pub planner_thoughts: Option<String>,
    pub tool_outcomes: Vec<ResponseToolOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseTurnOutput {
    pub message: String,
    pub referenced_ref_ids: BTreeSet<String>,
    pub summarized_ref_ids: BTreeSet<String>,
}

#[async_trait]
pub trait ResponseModel: Send + Sync {
    fn config(&self) -> &LlmModelConfig;

    async fn write_turn_response(
        &self,
        input: ResponseTurnInput,
    ) -> Result<ResponseTurnOutput, LlmError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRequest {
    pub run_id: RunId,
    pub ref_id: String,
    pub content: String,
    pub byte_count: u64,
    pub line_count: u64,
}

#[async_trait]
pub trait SummaryModel: Send + Sync {
    fn config(&self) -> &LlmModelConfig;

    async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError>;
}
