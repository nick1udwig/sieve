use crate::codex_auth::{
    read_openai_codex_auth_file, resolve_openai_codex_auth_json_path,
    OPENAI_CODEX_ACCESS_TOKEN_ENV, OPENAI_CODEX_ACCOUNT_ID_ENV, OPENAI_CODEX_AUTH_PATH_ENV,
};
use crate::LlmError;
use sieve_types::{LlmModelConfig, LlmProvider};
use std::env;
use std::path::PathBuf;

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
    let provider = maybe_upgrade_to_openai_codex(prefix, provider, api_base.as_deref(), getter)?;
    Ok(LlmModelConfig {
        provider,
        model,
        api_base,
    })
}

#[cfg(test)]
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
        LlmProvider::OpenAi | LlmProvider::OpenAiCodex => Ok(()),
    }
}

fn parse_provider(raw: &str) -> Result<LlmProvider, &'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "openai" | "open_ai" => Ok(LlmProvider::OpenAi),
        "openai-codex" | "openai_codex" => Ok(LlmProvider::OpenAiCodex),
        _ => Err("unsupported provider"),
    }
}

fn maybe_upgrade_to_openai_codex(
    prefix: &str,
    provider: LlmProvider,
    api_base: Option<&str>,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<LlmProvider, LlmError> {
    if provider != LlmProvider::OpenAi {
        return Ok(provider);
    }
    if api_base.is_some() || openai_api_key_present(prefix, getter) {
        return Ok(provider);
    }
    if openai_codex_auth_available(prefix, getter)? {
        return Ok(LlmProvider::OpenAiCodex);
    }
    Ok(provider)
}

fn openai_api_key_present(prefix: &str, getter: &dyn Fn(&str) -> Option<String>) -> bool {
    first_non_empty(getter(&format!("{prefix}_OPENAI_API_KEY")))
        .or_else(|| first_non_empty(getter("OPENAI_API_KEY")))
        .is_some()
}

fn openai_codex_auth_available(
    prefix: &str,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<bool, LlmError> {
    let scoped_access = first_non_empty(getter(&format!("{prefix}_OPENAI_CODEX_ACCESS_TOKEN")));
    let global_access = first_non_empty(getter(OPENAI_CODEX_ACCESS_TOKEN_ENV));
    if scoped_access.is_some() || global_access.is_some() {
        return Ok(
            first_non_empty(getter(&format!("{prefix}_OPENAI_CODEX_ACCOUNT_ID")))
                .or_else(|| first_non_empty(getter(OPENAI_CODEX_ACCOUNT_ID_ENV)))
                .is_some(),
        );
    }

    let auth_json_path = first_non_empty(getter(&format!("{prefix}_OPENAI_CODEX_AUTH_JSON_PATH")))
        .or_else(|| first_non_empty(getter(OPENAI_CODEX_AUTH_PATH_ENV)))
        .map(PathBuf::from)
        .unwrap_or_else(|| resolve_openai_codex_auth_json_path(getter));
    match read_openai_codex_auth_file(&auth_json_path) {
        Ok(_) => Ok(true),
        Err(LlmError::Config(_)) => Ok(false),
        Err(err) => Err(err),
    }
}

fn first_non_empty(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn env_getter(key: &str) -> Option<String> {
    env::var(key).ok()
}
