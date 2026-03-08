use serde_json::{json, Value};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub(crate) struct LlmExchangeLogger {
    path: Option<PathBuf>,
    provider: &'static str,
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
        let event = json!({
            "event": "llm_provider_exchange",
            "schema_version": 1,
            "provider": self.provider,
            "created_at_ms": now_ms(),
            "endpoint": endpoint,
            "attempt": attempt,
            "request_json": request_json,
            "response_status": status,
            "response_body": response_body,
        });
        self.append(event);
    }

    pub(crate) fn log_transport_error(
        &self,
        endpoint: &str,
        request_json: &Value,
        attempt: usize,
        error: &str,
    ) {
        let event = json!({
            "event": "llm_provider_exchange",
            "schema_version": 1,
            "provider": self.provider,
            "created_at_ms": now_ms(),
            "endpoint": endpoint,
            "attempt": attempt,
            "request_json": request_json,
            "transport_error": error,
        });
        self.append(event);
    }

    fn append(&self, event: Value) {
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
