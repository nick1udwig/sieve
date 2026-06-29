use std::env;

const DEFAULT_CODEX_PROGRAM: &str = "codex";
pub(super) const DEFAULT_CODEX_CONNECT_TIMEOUT_MS: u64 = 5 * 1000;
const DEFAULT_TURN_TIMEOUT_MS: u64 = 15 * 60 * 1000;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30 * 1000;

#[derive(Debug, Clone)]
pub(super) struct CodexManagerConfig {
    pub(super) program: Option<String>,
    pub(super) ws_url: Option<String>,
    pub(super) connect_timeout_ms: u64,
    pub(super) model: Option<String>,
    pub(super) turn_timeout_ms: u64,
    pub(super) request_timeout_ms: u64,
}

impl CodexManagerConfig {
    pub(super) fn from_env() -> Self {
        Self {
            program: env_string("SIEVE_CODEX_APP_SERVER_BIN")
                .or_else(|| Some(DEFAULT_CODEX_PROGRAM.to_string())),
            ws_url: env_string("SIEVE_CODEX_APP_SERVER_WS_URL"),
            connect_timeout_ms: env_u64(
                "SIEVE_CODEX_APP_SERVER_CONNECT_TIMEOUT_MS",
                DEFAULT_CODEX_CONNECT_TIMEOUT_MS,
            ),
            model: env_string("SIEVE_CODEX_MODEL"),
            turn_timeout_ms: env_u64("SIEVE_CODEX_TURN_TIMEOUT_MS", DEFAULT_TURN_TIMEOUT_MS),
            request_timeout_ms: env_u64(
                "SIEVE_CODEX_REQUEST_TIMEOUT_MS",
                DEFAULT_REQUEST_TIMEOUT_MS,
            ),
        }
    }
}

fn env_string(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env_string(key)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
