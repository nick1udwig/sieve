use crate::LlmError;
use sieve_types::{LlmModelConfig, LlmProvider};
use std::env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmConfigs {
    pub planner: LlmModelConfig,
    pub guidance: LlmModelConfig,
}

impl LlmConfigs {
    pub fn from_env() -> Result<Self, LlmError> {
        Ok(Self {
            planner: load_model_config_from_env("SIEVE_PLANNER", &env_getter)?,
            guidance: load_model_config_from_env("SIEVE_GUIDANCE", &env_getter)
                .or_else(|_| load_model_config_from_env("SIEVE_PLANNER", &env_getter))?,
        })
    }
}

pub(crate) fn load_model_config_from_env(
    prefix: &str,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<LlmModelConfig, LlmError> {
    let model_key = format!("{prefix}_MODEL");
    let model = getter(&model_key).ok_or_else(|| {
        LlmError::Config(format!(
            "missing required environment variable `{model_key}`"
        ))
    })?;

    if model.trim().is_empty() {
        return Err(LlmError::Config(format!(
            "environment variable `{model_key}` must not be empty"
        )));
    }

    let provider_key = format!("{prefix}_PROVIDER");
    let provider_value = getter(&provider_key).unwrap_or_else(|| "openai".to_string());
    let provider = parse_provider(&provider_value)
        .map_err(|msg| LlmError::Config(format!("{msg}; set `{provider_key}` to `openai`")))?;

    let api_base = getter(&format!("{prefix}_API_BASE")).and_then(|raw| {
        let value = raw.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    });
    Ok(LlmModelConfig {
        provider,
        model,
        api_base,
    })
}

pub(crate) fn load_openai_api_key_from_env(
    prefix: &str,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<String, LlmError> {
    let scoped_key_name = format!("{prefix}_OPENAI_API_KEY");
    let api_key = getter(&scoped_key_name)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| getter("OPENAI_API_KEY").filter(|value| !value.trim().is_empty()));
    match api_key {
        Some(key) if !key.trim().is_empty() => Ok(key),
        _ => Err(LlmError::Config(format!(
            "missing OpenAI API key; set `{scoped_key_name}` or `OPENAI_API_KEY`"
        ))),
    }
}

pub(crate) fn ensure_provider_openai(config: &LlmModelConfig) -> Result<(), LlmError> {
    match config.provider {
        LlmProvider::OpenAi => Ok(()),
    }
}

fn parse_provider(raw: &str) -> Result<LlmProvider, &'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "openai" | "open_ai" => Ok(LlmProvider::OpenAi),
        _ => Err("unsupported provider"),
    }
}

pub(crate) fn env_getter(key: &str) -> Option<String> {
    env::var(key).ok()
}
