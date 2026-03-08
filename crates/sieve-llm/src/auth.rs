use crate::codex_auth::{
    read_openai_codex_auth_file, refresh_openai_codex_access_token,
    resolve_openai_codex_auth_json_path, write_openai_codex_auth_file, OpenAiCodexStoredAuth,
    OPENAI_CODEX_ACCESS_TOKEN_ENV, OPENAI_CODEX_ACCOUNT_ID_ENV, OPENAI_CODEX_AUTH_PATH_ENV,
};
use crate::LlmError;
use reqwest::Client;
use sieve_types::LlmProvider;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub(crate) enum ProviderAuth {
    OpenAi { api_key: String },
    OpenAiCodex(OpenAiCodexAuth),
}

#[derive(Debug, Clone)]
pub(crate) struct OpenAiCodexAuth {
    state: Arc<Mutex<OpenAiCodexAuthState>>,
}

#[derive(Debug, Clone)]
struct OpenAiCodexAuthState {
    access_token: String,
    account_id: String,
    refresh_token: Option<String>,
    expires_at_ms: Option<u64>,
    auth_json_path: Option<PathBuf>,
}

impl OpenAiCodexAuth {
    fn new(auth: OpenAiCodexStoredAuth, auth_json_path: Option<PathBuf>) -> Self {
        Self {
            state: Arc::new(Mutex::new(OpenAiCodexAuthState {
                access_token: auth.access_token,
                account_id: auth.account_id,
                refresh_token: auth.refresh_token,
                expires_at_ms: auth.expires_at_ms,
                auth_json_path,
            })),
        }
    }

    pub(crate) async fn access_token_and_account_id(
        &self,
        http: &Client,
    ) -> Result<(String, String), LlmError> {
        if self.needs_refresh().await {
            self.refresh(http).await?;
        }

        let state = self.state.lock().await;
        Ok((state.access_token.clone(), state.account_id.clone()))
    }

    async fn needs_refresh(&self) -> bool {
        let state = self.state.lock().await;
        let Some(expires_at_ms) = state.expires_at_ms else {
            return false;
        };
        now_ms() >= expires_at_ms
    }

    async fn refresh(&self, http: &Client) -> Result<(), LlmError> {
        let (refresh_token, auth_json_path) = {
            let state = self.state.lock().await;
            let refresh_token = state.refresh_token.clone().ok_or_else(|| {
                LlmError::Config(
                    "openai_codex credentials are expired and missing refresh_token".to_string(),
                )
            })?;
            (refresh_token, state.auth_json_path.clone())
        };

        let refreshed = refresh_openai_codex_access_token(http, &refresh_token).await?;

        {
            let mut state = self.state.lock().await;
            state.access_token = refreshed.access_token.clone();
            state.account_id = refreshed.account_id.clone();
            state.refresh_token = refreshed.refresh_token.clone();
            state.expires_at_ms = refreshed.expires_at_ms;
        }

        if let Some(path) = auth_json_path {
            if let Err(err) = write_openai_codex_auth_file(&path, &refreshed) {
                eprintln!(
                    "sieve-llm failed writing refreshed openai_codex auth file {}: {}",
                    path.display(),
                    err
                );
            }
        }

        Ok(())
    }
}

pub(crate) fn load_provider_auth(
    prefixes: &[&str],
    provider: LlmProvider,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<ProviderAuth, LlmError> {
    match provider {
        LlmProvider::OpenAi => Ok(ProviderAuth::OpenAi {
            api_key: load_openai_api_key(prefixes, getter)?,
        }),
        LlmProvider::OpenAiCodex => {
            let auth = load_openai_codex_auth(prefixes, getter)?;
            Ok(ProviderAuth::OpenAiCodex(auth))
        }
    }
}

fn load_openai_api_key(
    prefixes: &[&str],
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<String, LlmError> {
    for prefix in prefixes {
        let key_name = format!("{prefix}_OPENAI_API_KEY");
        if let Some(key) = getter(&key_name).filter(|value| !value.trim().is_empty()) {
            return Ok(key);
        }
    }

    getter("OPENAI_API_KEY")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            let first_prefix = prefixes.first().copied().unwrap_or("SIEVE_PLANNER");
            let scoped_key_name = format!("{first_prefix}_OPENAI_API_KEY");
            LlmError::Config(format!(
                "missing OpenAI API key; set `{scoped_key_name}` or `OPENAI_API_KEY`"
            ))
        })
}

fn load_openai_codex_auth(
    prefixes: &[&str],
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<OpenAiCodexAuth, LlmError> {
    let access_token = first_non_empty(
        prefixes
            .iter()
            .map(|prefix| format!("{prefix}_OPENAI_CODEX_ACCESS_TOKEN"))
            .chain(std::iter::once(OPENAI_CODEX_ACCESS_TOKEN_ENV.to_string())),
        getter,
    );
    let account_id = first_non_empty(
        prefixes
            .iter()
            .map(|prefix| format!("{prefix}_OPENAI_CODEX_ACCOUNT_ID"))
            .chain(std::iter::once(OPENAI_CODEX_ACCOUNT_ID_ENV.to_string())),
        getter,
    );

    if let Some(access_token) = access_token {
        let account_id = account_id.ok_or_else(|| {
                let first_prefix = prefixes
                .first()
                .copied()
                .unwrap_or("SIEVE_PLANNER");
            LlmError::Config(format!(
                "missing openai_codex account id; set `{first_prefix}_OPENAI_CODEX_ACCOUNT_ID` or `{OPENAI_CODEX_ACCOUNT_ID_ENV}`"
            ))
        })?;
        return Ok(OpenAiCodexAuth::new(
            OpenAiCodexStoredAuth {
                access_token,
                account_id,
                refresh_token: None,
                expires_at_ms: None,
            },
            None,
        ));
    }

    let auth_json_path = first_non_empty(
        prefixes
            .iter()
            .map(|prefix| format!("{prefix}_OPENAI_CODEX_AUTH_JSON_PATH"))
            .chain(std::iter::once(OPENAI_CODEX_AUTH_PATH_ENV.to_string())),
        getter,
    )
    .map(PathBuf::from)
    .unwrap_or_else(|| resolve_openai_codex_auth_json_path(getter));

    let stored = read_openai_codex_auth_file(&auth_json_path)?;
    Ok(OpenAiCodexAuth::new(stored, Some(auth_json_path)))
}

fn first_non_empty<I>(keys: I, getter: &dyn Fn(&str) -> Option<String>) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    keys.into_iter()
        .find_map(|key| getter(&key).map(|value| value.trim().to_string()))
        .filter(|value| !value.is_empty())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
