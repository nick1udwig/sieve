use serde::Serialize;
use serde_json::Value;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub(crate) struct LlmExchangeLogger {
    path: Option<PathBuf>,
    provider: &'static str,
}

#[derive(Serialize)]
struct ProviderExchangeEvent<'a> {
    event: &'static str,
    schema_version: u8,
    provider: &'a str,
    created_at_ms: u64,
    endpoint: &'a str,
    attempt: usize,
    request_json: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport_error: Option<&'a str>,
}

impl LlmExchangeLogger {
    pub(crate) fn with_path(path: Option<PathBuf>, provider: &'static str) -> Self {
        Self { path, provider }
    }

    pub(crate) fn from_env(provider: &'static str) -> Self {
        if let Some(explicit) = std::env::var("SIEVE_LLM_EXCHANGE_LOG_PATH")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
        {
            return Self::with_path(Some(PathBuf::from(explicit)), provider);
        }

        let sieve_home = std::env::var("SIEVE_HOME")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|raw| raw.trim().to_string())
                    .filter(|raw| !raw.is_empty())
                    .map(PathBuf::from)
                    .map(|home| home.join(".sieve"))
            });

        let default_path = sieve_home.map(|home| home.join("logs/llm-provider-exchanges.jsonl"));
        Self::with_path(default_path, provider)
    }

    pub(crate) fn log_http(
        &self,
        endpoint: &str,
        request_json: &Value,
        attempt: usize,
        status: u16,
        response_body: &str,
    ) {
        self.append(ProviderExchangeEvent {
            event: "llm_provider_exchange",
            schema_version: 1,
            provider: self.provider,
            created_at_ms: now_ms(),
            endpoint,
            attempt,
            request_json,
            response_status: Some(status),
            response_body: Some(response_body),
            transport_error: None,
        });
    }

    pub(crate) fn log_transport_error(
        &self,
        endpoint: &str,
        request_json: &Value,
        attempt: usize,
        error: &str,
    ) {
        self.append(ProviderExchangeEvent {
            event: "llm_provider_exchange",
            schema_version: 1,
            provider: self.provider,
            created_at_ms: now_ms(),
            endpoint,
            attempt,
            request_json,
            response_status: None,
            response_body: None,
            transport_error: Some(error),
        });
    }

    fn append<T: Serialize>(&self, event: T) {
        let Some(path) = &self.path else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = create_dir_all(parent) {
                eprintln!(
                    "sieve-llm exchange logger failed creating {}: {}",
                    parent.display(),
                    err
                );
                return;
            }
        }
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
            eprintln!(
                "sieve-llm exchange logger failed opening {}",
                path.display()
            );
            return;
        };
        let Ok(line) = serde_json::to_string(&event) else {
            eprintln!("sieve-llm exchange logger failed serializing event");
            return;
        };
        if let Err(err) = writeln!(file, "{line}") {
            eprintln!(
                "sieve-llm exchange logger failed writing {}: {}",
                path.display(),
                err
            );
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
