use crate::{PlannerGuidanceFrame, RunId, RuntimeEvent};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Supported LLM provider enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    OpenAi,
    OpenAiCodex,
}

/// Configuration for one planner model endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmModelConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub api_base: Option<String>,
}

/// Planner invocation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerBrowserSession {
    pub session_name: String,
    pub current_origin: String,
    pub current_url: String,
}

/// Planner invocation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerTurnInput {
    pub run_id: RunId,
    pub user_message: String,
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub current_time_utc: Option<String>,
    #[serde(default)]
    pub current_timezone: Option<String>,
    #[serde(default)]
    pub allowed_net_connect_scopes: Vec<String>,
    #[serde(default)]
    pub browser_sessions: Vec<PlannerBrowserSession>,
    pub previous_events: Vec<RuntimeEvent>,
    #[serde(default)]
    pub guidance: Option<PlannerGuidanceFrame>,
}

/// One tool call selected by planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerToolCall {
    pub tool_name: String,
    pub args: BTreeMap<String, serde_json::Value>,
}

/// Planner model output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerTurnOutput {
    pub thoughts: Option<String>,
    pub tool_calls: Vec<PlannerToolCall>,
}
