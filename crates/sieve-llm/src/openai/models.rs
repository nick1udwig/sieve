use super::client::OpenAiClient;
use super::planner_retry::{
    run_planner_with_one_regeneration, run_planner_with_one_regeneration_with_builder,
};
use super::requests::{build_guidance_request, build_response_request, build_summary_request};
use crate::auth::{load_provider_auth, CredentialBroker, ProviderAuth};
use crate::codex::{
    build_codex_guidance_request, build_codex_planner_request, build_codex_response_request,
    build_codex_summary_request, OpenAiCodexClient,
};
use crate::config::{
    ensure_provider_openai, env_getter, load_model_config_from_env,
    load_optional_model_config_from_env,
};
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

/// The four Sieve model roles assembled with a shared credential cache.
pub struct OpenAiModelSet {
    planner: OpenAiPlannerModel,
    guidance: OpenAiGuidanceModel,
    response: OpenAiResponseModel,
    summary: OpenAiSummaryModel,
}

impl OpenAiModelSet {
    /// Loads every model role from the environment and shares equivalent auth sources.
    pub fn from_env() -> Result<Self, LlmError> {
        Self::from_getter(&env_getter)
    }

    /// Returns planner, guidance, response, and summary models in that order.
    pub fn into_parts(
        self,
    ) -> (
        OpenAiPlannerModel,
        OpenAiGuidanceModel,
        OpenAiResponseModel,
        OpenAiSummaryModel,
    ) {
        (self.planner, self.guidance, self.response, self.summary)
    }

    fn from_getter(getter: &dyn Fn(&str) -> Option<String>) -> Result<Self, LlmError> {
        let planner_config = load_model_config_from_env("SIEVE_PLANNER", getter)?;
        let guidance_config =
            load_optional_model_config_from_env("SIEVE_GUIDANCE", &planner_config, getter)?;
        let response_config =
            load_optional_model_config_from_env("SIEVE_RESPONSE", &planner_config, getter)?;
        let summary_config =
            load_optional_model_config_from_env("SIEVE_QUARANTINE", &planner_config, getter)?;
        let broker = CredentialBroker::default();

        let planner_auth =
            broker.load_provider_auth(&["SIEVE_PLANNER"], planner_config.provider, getter)?;
        let guidance_auth = broker.load_provider_auth(
            &["SIEVE_GUIDANCE", "SIEVE_PLANNER"],
            guidance_config.provider,
            getter,
        )?;
        let response_auth = broker.load_provider_auth(
            &["SIEVE_RESPONSE", "SIEVE_PLANNER"],
            response_config.provider,
            getter,
        )?;
        let summary_auth = broker.load_provider_auth(
            &["SIEVE_QUARANTINE", "SIEVE_PLANNER"],
            summary_config.provider,
            getter,
        )?;

        Ok(Self {
            planner: OpenAiPlannerModel::new_with_auth(planner_config, planner_auth)?,
            guidance: OpenAiGuidanceModel::new_with_auth(guidance_config, guidance_auth)?,
            response: OpenAiResponseModel::new_with_auth(response_config, response_auth)?,
            summary: OpenAiSummaryModel::new_with_auth(summary_config, summary_auth)?,
        })
    }
}

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
        let planner_config = load_model_config_from_env("SIEVE_PLANNER", &env_getter)?;
        let config =
            load_optional_model_config_from_env("SIEVE_GUIDANCE", &planner_config, &env_getter)?;
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
        let planner_config = load_model_config_from_env("SIEVE_PLANNER", &env_getter)?;
        let config =
            load_optional_model_config_from_env("SIEVE_RESPONSE", &planner_config, &env_getter)?;
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
        let planner_config = load_model_config_from_env("SIEVE_PLANNER", &env_getter)?;
        let config =
            load_optional_model_config_from_env("SIEVE_QUARANTINE", &planner_config, &env_getter)?;
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

#[cfg(test)]
mod model_set_tests {
    use super::*;
    use crate::auth::OpenAiCodexAuth;
    use crate::codex_auth::{write_openai_codex_auth_file, OpenAiCodexStoredAuth};
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn map_getter<'a>(map: &'a BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| map.get(key).cloned()
    }

    fn codex_auth(client: &ProviderClient) -> &OpenAiCodexAuth {
        match client {
            ProviderClient::OpenAiCodex(client) => client.auth(),
            ProviderClient::OpenAi(_) => panic!("expected Codex client"),
        }
    }

    #[test]
    fn model_set_assembles_all_roles_with_one_shared_codex_auth_state() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("sieve-model-set-{nonce}"));
        let auth_path = root.join("auth.json");
        write_openai_codex_auth_file(
            &auth_path,
            &OpenAiCodexStoredAuth {
                access_token: "access".to_string(),
                account_id: "account".to_string(),
                refresh_token: Some("refresh".to_string()),
                expires_at_ms: Some(4_102_444_800_000),
            },
        )
        .expect("write auth file");
        let mut values = BTreeMap::new();
        values.insert("SIEVE_PLANNER_MODEL".to_string(), "gpt-5.4".to_string());
        values.insert(
            "SIEVE_PLANNER_PROVIDER".to_string(),
            "openai_codex".to_string(),
        );
        values.insert(
            "SIEVE_OPENAI_CODEX_AUTH_JSON_PATH".to_string(),
            auth_path.display().to_string(),
        );

        let models = OpenAiModelSet::from_getter(&map_getter(&values)).expect("model set");
        let planner = codex_auth(&models.planner.client);
        let guidance = codex_auth(&models.guidance.client);
        let response = codex_auth(&models.response.client);
        let summary = codex_auth(&models.summary.client);
        assert!(planner.shares_state_with(guidance));
        assert!(planner.shares_state_with(response));
        assert!(planner.shares_state_with(summary));

        let (planner, guidance, response, summary) = models.into_parts();
        assert_eq!(planner.config.model, "gpt-5.4");
        assert_eq!(guidance.config.model, "gpt-5.4");
        assert_eq!(response.config.model, "gpt-5.4");
        assert_eq!(summary.config.model, "gpt-5.4");

        let _ = fs::remove_dir_all(root);
    }
}
