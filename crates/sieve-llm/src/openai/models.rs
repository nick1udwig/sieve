use super::client::OpenAiClient;
use super::planner_retry::{
    run_planner_with_one_regeneration, run_planner_with_one_regeneration_with_builder,
};
use super::requests::{build_guidance_request, build_response_request, build_summary_request};
use crate::auth::{load_provider_auth, ProviderAuth};
use crate::codex::{
    build_codex_guidance_request, build_codex_planner_request, build_codex_response_request,
    build_codex_summary_request, OpenAiCodexClient,
};
use crate::config::{ensure_provider_openai, env_getter, load_model_config_from_env};
use crate::wire::{
    build_planner_messages, decode_guidance_output, decode_response_output,
    extract_openai_message_content_json,
};
use crate::{
    GuidanceModel, LlmError, PlannerModel, ResponseModel, ResponseTurnInput, ResponseTurnOutput,
    SummaryModel, SummaryRequest,
};
use async_trait::async_trait;
use sieve_types::{
    LlmModelConfig, LlmProvider, PlannerGuidanceInput, PlannerGuidanceOutput, PlannerTurnInput,
    PlannerTurnOutput,
};

pub struct OpenAiPlannerModel {
    config: LlmModelConfig,
    client: ProviderClient,
}

impl OpenAiPlannerModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_openai_client(&config, api_key, "planner OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub(crate) fn new_with_auth(
        config: LlmModelConfig,
        auth: ProviderAuth,
    ) -> Result<Self, LlmError> {
        let client = build_client(&config, auth, "planner OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_PLANNER", &env_getter)?;
        let auth = load_provider_auth(&["SIEVE_PLANNER"], config.provider, &env_getter)?;
        Self::new_with_auth(config, auth)
    }
}

#[async_trait]
impl PlannerModel for OpenAiPlannerModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        let messages = build_planner_messages(&input)?;

        match &self.client {
            ProviderClient::OpenAi(client) => {
                run_planner_with_one_regeneration(
                    self.config.model.as_str(),
                    messages,
                    &input.allowed_tools,
                    |request| client.create_chat_completion(request),
                )
                .await
            }
            ProviderClient::OpenAiCodex(client) => {
                run_planner_with_one_regeneration_with_builder(
                    self.config.model.as_str(),
                    messages,
                    &input.allowed_tools,
                    build_codex_planner_request,
                    |request| client.create_response(request),
                )
                .await
            }
        }
    }
}

pub struct OpenAiGuidanceModel {
    config: LlmModelConfig,
    client: ProviderClient,
}

impl OpenAiGuidanceModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_openai_client(&config, api_key, "guidance OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub(crate) fn new_with_auth(
        config: LlmModelConfig,
        auth: ProviderAuth,
    ) -> Result<Self, LlmError> {
        let client = build_client(&config, auth, "guidance OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_GUIDANCE", &env_getter)
            .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?;
        let auth = load_provider_auth(
            &["SIEVE_GUIDANCE", "SIEVE_PLANNER"],
            config.provider,
            &env_getter,
        )?;
        Self::new_with_auth(config, auth)
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
        let response = match &self.client {
            ProviderClient::OpenAi(client) => {
                client
                    .create_chat_completion(build_guidance_request(input, &self.config.model))
                    .await?
            }
            ProviderClient::OpenAiCodex(client) => {
                client
                    .create_response(build_codex_guidance_request(input, &self.config.model))
                    .await?
            }
        };
        let content_json = extract_openai_message_content_json(&response)?;
        decode_guidance_output(content_json)
    }
}

pub struct OpenAiResponseModel {
    config: LlmModelConfig,
    client: ProviderClient,
}

impl OpenAiResponseModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_openai_client(&config, api_key, "response OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub(crate) fn new_with_auth(
        config: LlmModelConfig,
        auth: ProviderAuth,
    ) -> Result<Self, LlmError> {
        let client = build_client(&config, auth, "response OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_RESPONSE", &env_getter)
            .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?;
        let auth = load_provider_auth(
            &["SIEVE_RESPONSE", "SIEVE_PLANNER"],
            config.provider,
            &env_getter,
        )?;
        Self::new_with_auth(config, auth)
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
        let response = match &self.client {
            ProviderClient::OpenAi(client) => {
                client
                    .create_chat_completion(build_response_request(&input, &self.config.model)?)
                    .await?
            }
            ProviderClient::OpenAiCodex(client) => {
                client
                    .create_response(build_codex_response_request(&input, &self.config.model)?)
                    .await?
            }
        };
        let content_json = extract_openai_message_content_json(&response)?;
        decode_response_output(content_json)
    }
}

pub struct OpenAiSummaryModel {
    config: LlmModelConfig,
    client: ProviderClient,
}

impl OpenAiSummaryModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        let client = build_openai_client(&config, api_key, "summary OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub(crate) fn new_with_auth(
        config: LlmModelConfig,
        auth: ProviderAuth,
    ) -> Result<Self, LlmError> {
        let client = build_client(&config, auth, "summary OpenAI API key is empty")?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_QUARANTINE", &env_getter)
            .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?;
        let auth = load_provider_auth(
            &["SIEVE_QUARANTINE", "SIEVE_PLANNER"],
            config.provider,
            &env_getter,
        )?;
        Self::new_with_auth(config, auth)
    }
}

#[async_trait]
impl SummaryModel for OpenAiSummaryModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError> {
        let response = match &self.client {
            ProviderClient::OpenAi(client) => {
                client
                    .create_chat_completion(build_summary_request(request, &self.config.model))
                    .await?
            }
            ProviderClient::OpenAiCodex(client) => {
                client
                    .create_response(build_codex_summary_request(request, &self.config.model))
                    .await?
            }
        };
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

enum ProviderClient {
    OpenAi(OpenAiClient),
    OpenAiCodex(OpenAiCodexClient),
}

fn build_openai_client(
    config: &LlmModelConfig,
    api_key: String,
    empty_api_key_message: &str,
) -> Result<ProviderClient, LlmError> {
    build_client(
        config,
        ProviderAuth::OpenAi { api_key },
        empty_api_key_message,
    )
}

fn build_client(
    config: &LlmModelConfig,
    auth: ProviderAuth,
    empty_api_key_message: &str,
) -> Result<ProviderClient, LlmError> {
    ensure_provider_openai(config)?;
    match (config.provider, auth) {
        (LlmProvider::OpenAi, ProviderAuth::OpenAi { api_key }) => {
            if api_key.trim().is_empty() {
                return Err(LlmError::Config(empty_api_key_message.to_string()));
            }
            Ok(ProviderClient::OpenAi(OpenAiClient::new(
                api_key,
                config.api_base.clone(),
            )?))
        }
        (LlmProvider::OpenAiCodex, ProviderAuth::OpenAiCodex(auth)) => Ok(
            ProviderClient::OpenAiCodex(OpenAiCodexClient::new(auth, config.api_base.clone())?),
        ),
        (LlmProvider::OpenAi, ProviderAuth::OpenAiCodex(_)) => Err(LlmError::Config(
            "provider/auth mismatch: `openai` model cannot use openai_codex credentials"
                .to_string(),
        )),
        (LlmProvider::OpenAiCodex, ProviderAuth::OpenAi { .. }) => Err(LlmError::Config(
            "provider/auth mismatch: `openai_codex` model requires openai_codex credentials"
                .to_string(),
        )),
    }
}
