use crate::clock::now_ms;
use crate::codex_auth::{
    read_openai_codex_auth_file, refresh_openai_codex_access_token,
    resolve_openai_codex_auth_json_path, trim_to_non_empty, write_openai_codex_auth_file,
    OpenAiCodexStoredAuth, OPENAI_CODEX_ACCESS_TOKEN_ENV, OPENAI_CODEX_ACCOUNT_ID_ENV,
    OPENAI_CODEX_AUTH_PATH_ENV,
};
use crate::LlmError;
use reqwest::Client;
use sieve_types::LlmProvider;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as SyncMutex;
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
        self.access_token_and_account_id_with(|refresh_token| async move {
            refresh_openai_codex_access_token(http, &refresh_token).await
        })
        .await
    }

    async fn access_token_and_account_id_with<F, Fut>(
        &self,
        refresh: F,
    ) -> Result<(String, String), LlmError>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<OpenAiCodexStoredAuth, LlmError>>,
    {
        let mut state = self.state.lock().await;
        if state
            .expires_at_ms
            .is_some_and(|expires_at_ms| now_ms() >= expires_at_ms)
        {
            let refresh_token = state.refresh_token.clone().ok_or_else(|| {
                LlmError::Config(
                    "openai_codex credentials are expired and missing refresh_token".to_string(),
                )
            })?;
            let auth_json_path = state.auth_json_path.clone();
            let refreshed = refresh(refresh_token).await?;
            state.access_token = refreshed.access_token.clone();
            state.account_id = refreshed.account_id.clone();
            state.refresh_token = refreshed.refresh_token.clone();
            state.expires_at_ms = refreshed.expires_at_ms;
            if let Some(path) = auth_json_path {
                if let Err(err) = write_openai_codex_auth_file(&path, &refreshed) {
                    eprintln!(
                        "sieve-llm failed writing refreshed openai_codex auth file {}: {}",
                        path.display(),
                        err
                    );
                }
            }
        }
        Ok((state.access_token.clone(), state.account_id.clone()))
    }

    #[cfg(test)]
    pub(crate) fn shares_state_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum OpenAiCodexAuthSource {
    Environment {
        access_token_key: String,
        account_id_key: String,
    },
    File(PathBuf),
}

enum ResolvedOpenAiCodexAuth {
    Environment {
        source: OpenAiCodexAuthSource,
        auth: OpenAiCodexStoredAuth,
    },
    File {
        source: OpenAiCodexAuthSource,
        path: PathBuf,
    },
}

impl ResolvedOpenAiCodexAuth {
    fn source(&self) -> &OpenAiCodexAuthSource {
        match self {
            Self::Environment { source, .. } | Self::File { source, .. } => source,
        }
    }

    fn load(self) -> Result<OpenAiCodexAuth, LlmError> {
        match self {
            Self::Environment { auth, .. } => Ok(OpenAiCodexAuth::new(auth, None)),
            Self::File { path, .. } => {
                let stored = read_openai_codex_auth_file(&path)?;
                Ok(OpenAiCodexAuth::new(stored, Some(path)))
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct CredentialBroker {
    openai_codex: SyncMutex<HashMap<OpenAiCodexAuthSource, OpenAiCodexAuth>>,
}

impl CredentialBroker {
    pub(crate) fn load_provider_auth(
        &self,
        prefixes: &[&str],
        provider: LlmProvider,
        getter: &dyn Fn(&str) -> Option<String>,
    ) -> Result<ProviderAuth, LlmError> {
        match provider {
            LlmProvider::OpenAi => Ok(ProviderAuth::OpenAi {
                api_key: load_openai_api_key(prefixes, getter)?,
            }),
            LlmProvider::OpenAiCodex => {
                let resolved = resolve_openai_codex_auth(prefixes, getter)?;
                let source = resolved.source().clone();
                let mut cache = self.openai_codex.lock().map_err(|_| {
                    LlmError::Config("OpenAI Codex credential cache lock poisoned".to_string())
                })?;
                let auth = if let Some(auth) = cache.get(&source) {
                    auth.clone()
                } else {
                    let auth = resolved.load()?;
                    cache.insert(source, auth.clone());
                    auth
                };
                Ok(ProviderAuth::OpenAiCodex(auth))
            }
        }
    }
}

pub(crate) fn load_provider_auth(
    prefixes: &[&str],
    provider: LlmProvider,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<ProviderAuth, LlmError> {
    CredentialBroker::default().load_provider_auth(prefixes, provider, getter)
}

fn load_openai_api_key(
    prefixes: &[&str],
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<String, LlmError> {
    first_non_empty(
        prefixes
            .iter()
            .map(|prefix| format!("{prefix}_OPENAI_API_KEY"))
            .chain(std::iter::once("OPENAI_API_KEY".to_string())),
        getter,
    )
    .ok_or_else(|| {
        let first_prefix = prefixes.first().copied().unwrap_or("SIEVE_PLANNER");
        let scoped_key_name = format!("{first_prefix}_OPENAI_API_KEY");
        LlmError::Config(format!(
            "missing OpenAI API key; set `{scoped_key_name}` or `OPENAI_API_KEY`"
        ))
    })
}

fn resolve_openai_codex_auth(
    prefixes: &[&str],
    getter: &dyn Fn(&str) -> Option<String>,
) -> Result<ResolvedOpenAiCodexAuth, LlmError> {
    let access_token = first_non_empty_entry(
        prefixes
            .iter()
            .map(|prefix| format!("{prefix}_OPENAI_CODEX_ACCESS_TOKEN"))
            .chain(std::iter::once(OPENAI_CODEX_ACCESS_TOKEN_ENV.to_string())),
        getter,
    );
    let account_id = first_non_empty_entry(
        prefixes
            .iter()
            .map(|prefix| format!("{prefix}_OPENAI_CODEX_ACCOUNT_ID"))
            .chain(std::iter::once(OPENAI_CODEX_ACCOUNT_ID_ENV.to_string())),
        getter,
    );

    if let Some((access_token_key, access_token)) = access_token {
        let (account_id_key, account_id) = account_id.ok_or_else(|| {
                let first_prefix = prefixes
                .first()
                .copied()
                .unwrap_or("SIEVE_PLANNER");
            LlmError::Config(format!(
                "missing openai_codex account id; set `{first_prefix}_OPENAI_CODEX_ACCOUNT_ID` or `{OPENAI_CODEX_ACCOUNT_ID_ENV}`"
            ))
        })?;
        return Ok(ResolvedOpenAiCodexAuth::Environment {
            source: OpenAiCodexAuthSource::Environment {
                access_token_key,
                account_id_key,
            },
            auth: OpenAiCodexStoredAuth {
                access_token,
                account_id,
                refresh_token: None,
                expires_at_ms: None,
            },
        });
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
    let path = normalize_auth_path(&auth_json_path);
    Ok(ResolvedOpenAiCodexAuth::File {
        source: OpenAiCodexAuthSource::File(path.clone()),
        path,
    })
}

fn normalize_auth_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
}

fn first_non_empty<I>(keys: I, getter: &dyn Fn(&str) -> Option<String>) -> Option<String>
where
    I: IntoIterator<Item = String>,
{
    first_non_empty_entry(keys, getter).map(|(_, value)| value)
}

fn first_non_empty_entry<I>(
    keys: I,
    getter: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, String)>
where
    I: IntoIterator<Item = String>,
{
    keys.into_iter()
        .find_map(|key| trim_to_non_empty(getter(&key)).map(|value| (key, value)))
}

#[cfg(test)]
mod credential_broker_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn map_getter<'a>(map: &'a BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| map.get(key).cloned()
    }

    fn temp_auth_file(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir()
            .join(format!("sieve-credential-broker-{label}-{nonce}"))
            .join("auth.json");
        write_openai_codex_auth_file(
            &path,
            &OpenAiCodexStoredAuth {
                access_token: format!("access-{label}"),
                account_id: format!("account-{label}"),
                refresh_token: Some(format!("refresh-{label}")),
                expires_at_ms: Some(4_102_444_800_000),
            },
        )
        .expect("write auth file");
        path
    }

    fn codex_auth(auth: ProviderAuth) -> OpenAiCodexAuth {
        match auth {
            ProviderAuth::OpenAiCodex(auth) => auth,
            ProviderAuth::OpenAi { .. } => panic!("expected Codex auth"),
        }
    }

    #[test]
    fn credential_broker_shares_state_for_same_effective_auth_source_across_roles() {
        let auth_path = temp_auth_file("shared");
        let mut values = BTreeMap::new();
        values.insert(
            OPENAI_CODEX_AUTH_PATH_ENV.to_string(),
            auth_path.display().to_string(),
        );
        let getter = map_getter(&values);
        let broker = CredentialBroker::default();

        let planner = codex_auth(
            broker
                .load_provider_auth(&["SIEVE_PLANNER"], LlmProvider::OpenAiCodex, &getter)
                .expect("planner auth"),
        );
        for prefixes in [
            &["SIEVE_GUIDANCE", "SIEVE_PLANNER"][..],
            &["SIEVE_RESPONSE", "SIEVE_PLANNER"][..],
            &["SIEVE_QUARANTINE", "SIEVE_PLANNER"][..],
        ] {
            let role = codex_auth(
                broker
                    .load_provider_auth(prefixes, LlmProvider::OpenAiCodex, &getter)
                    .expect("role auth"),
            );
            assert!(Arc::ptr_eq(&planner.state, &role.state));
        }

        let _ = fs::remove_dir_all(auth_path.parent().expect("parent"));
    }

    #[test]
    fn credential_broker_keeps_distinct_auth_sources_isolated() {
        let planner_path = temp_auth_file("planner");
        let response_path = temp_auth_file("response");
        let mut values = BTreeMap::new();
        values.insert(
            "SIEVE_PLANNER_OPENAI_CODEX_AUTH_JSON_PATH".to_string(),
            planner_path.display().to_string(),
        );
        values.insert(
            "SIEVE_RESPONSE_OPENAI_CODEX_AUTH_JSON_PATH".to_string(),
            response_path.display().to_string(),
        );
        let getter = map_getter(&values);
        let broker = CredentialBroker::default();

        let planner = codex_auth(
            broker
                .load_provider_auth(&["SIEVE_PLANNER"], LlmProvider::OpenAiCodex, &getter)
                .expect("planner auth"),
        );
        let response = codex_auth(
            broker
                .load_provider_auth(
                    &["SIEVE_RESPONSE", "SIEVE_PLANNER"],
                    LlmProvider::OpenAiCodex,
                    &getter,
                )
                .expect("response auth"),
        );

        assert!(!Arc::ptr_eq(&planner.state, &response.state));

        let _ = fs::remove_dir_all(planner_path.parent().expect("parent"));
        let _ = fs::remove_dir_all(response_path.parent().expect("parent"));
    }

    #[tokio::test]
    async fn shared_auth_single_flights_concurrent_refreshes() {
        let auth = OpenAiCodexAuth::new(
            OpenAiCodexStoredAuth {
                access_token: "expired-access".to_string(),
                account_id: "account".to_string(),
                refresh_token: Some("refresh".to_string()),
                expires_at_ms: Some(0),
            },
            None,
        );
        let refresh_count = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let auth = auth.clone();
            let refresh_count = refresh_count.clone();
            tasks.push(tokio::spawn(async move {
                auth.access_token_and_account_id_with(move |refresh_token| async move {
                    assert_eq!(refresh_token, "refresh");
                    refresh_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(OpenAiCodexStoredAuth {
                        access_token: "fresh-access".to_string(),
                        account_id: "account".to_string(),
                        refresh_token: Some("fresh-refresh".to_string()),
                        expires_at_ms: Some(4_102_444_800_000),
                    })
                })
                .await
            }));
        }

        for task in tasks {
            assert_eq!(
                task.await.expect("join").expect("auth"),
                ("fresh-access".to_string(), "account".to_string())
            );
        }
        assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    }
}
