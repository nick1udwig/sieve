use super::client::OpenAiClient;
use super::planner_retry::run_planner_with_one_regeneration;
use super::requests::{build_guidance_request, build_response_request, build_summary_request};
use crate::config::{
    ensure_provider_openai, env_getter, load_model_config_from_env, load_openai_api_key_from_env,
};
use crate::wire::{
    decode_guidance_output, decode_response_output, extract_openai_message_content_json,
    serialize_planner_input, PLANNER_SYSTEM_PROMPT,
};
use crate::{
    GuidanceModel, LlmError, PlannerModel, ResponseModel, ResponseTurnInput, ResponseTurnOutput,
    SummaryModel, SummaryRequest,
};
use async_trait::async_trait;
use serde_json::json;
use sieve_types::{
    LlmModelConfig, PlannerGuidanceInput, PlannerGuidanceOutput, PlannerTurnInput,
    PlannerTurnOutput,
};

pub struct OpenAiPlannerModel {
    config: LlmModelConfig,
    client: OpenAiClient,
}

impl OpenAiPlannerModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_client(&config, api_key, "planner OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_PLANNER", &env_getter)?;
        let api_key = load_openai_api_key_from_env("SIEVE_PLANNER", &env_getter)?;
        Self::new(config, api_key)
    }
}

#[async_trait]
impl PlannerModel for OpenAiPlannerModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        let planner_payload = serialize_planner_input(&input)?;
        let messages = vec![
            json!({"role":"system","content": PLANNER_SYSTEM_PROMPT}),
            json!({"role":"user","content": planner_payload.to_string()}),
        ];

        run_planner_with_one_regeneration(
            self.config.model.as_str(),
            messages,
            &input.allowed_tools,
            |request| self.client.create_chat_completion(request),
        )
        .await
    }
}

pub struct OpenAiGuidanceModel {
    config: LlmModelConfig,
    client: OpenAiClient,
}

impl OpenAiGuidanceModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_client(&config, api_key, "guidance OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_GUIDANCE", &env_getter)
            .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?;
        let api_key = load_openai_api_key_from_env("SIEVE_GUIDANCE", &env_getter)
            .or_else(|_| load_openai_api_key_from_env("SIEVE_PLANNER", &env_getter))?;
        Self::new(config, api_key)
    }
}

#[async_trait]
impl GuidanceModel for OpenAiGuidanceModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn classify_guidance(
        &self,
        input: PlannerGuidanceInput,
    ) -> Result<PlannerGuidanceOutput, LlmError> {
        let response = self
            .client
            .create_chat_completion(build_guidance_request(input, &self.config.model))
            .await?;
        let content_json = extract_openai_message_content_json(&response)?;
        decode_guidance_output(content_json)
    }
}

pub struct OpenAiResponseModel {
    config: LlmModelConfig,
    client: OpenAiClient,
}

impl OpenAiResponseModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_client(&config, api_key, "response OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_RESPONSE", &env_getter)
            .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?;
        let api_key = load_openai_api_key_from_env("SIEVE_RESPONSE", &env_getter)
            .or_else(|_| load_openai_api_key_from_env("SIEVE_PLANNER", &env_getter))?;
        Self::new(config, api_key)
    }
}

#[async_trait]
impl ResponseModel for OpenAiResponseModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn write_turn_response(
        &self,
        input: ResponseTurnInput,
    ) -> Result<ResponseTurnOutput, LlmError> {
        let response = self
            .client
            .create_chat_completion(build_response_request(&input, &self.config.model)?)
            .await?;
        let content_json = extract_openai_message_content_json(&response)?;
        decode_response_output(content_json)
    }
}

pub struct OpenAiSummaryModel {
    config: LlmModelConfig,
    client: OpenAiClient,
}

impl OpenAiSummaryModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_client(&config, api_key, "summary OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_QUARANTINE", &env_getter)
            .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?;
        let api_key = load_openai_api_key_from_env("SIEVE_QUARANTINE", &env_getter)
            .or_else(|_| load_openai_api_key_from_env("SIEVE_PLANNER", &env_getter))?;
        Self::new(config, api_key)
    }
}

#[async_trait]
impl SummaryModel for OpenAiSummaryModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError> {
        let response = self
            .client
            .create_chat_completion(build_summary_request(request, &self.config.model))
            .await?;
        let content_json = extract_openai_message_content_json(&response)?;
        let summary = content_json
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| LlmError::Decode("summary output missing `summary`".to_string()))?;
        Ok(summary.to_string())
    }
}

fn build_client(
    config: &LlmModelConfig,
    api_key: String,
    empty_api_key_message: &str,
) -> Result<OpenAiClient, LlmError> {
    ensure_provider_openai(config)?;
    if api_key.trim().is_empty() {
        return Err(LlmError::Config(empty_api_key_message.to_string()));
    }
    OpenAiClient::new(api_key, config.api_base.clone())
}
