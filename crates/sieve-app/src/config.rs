use crate::automation::{parse_duration_ms, DEFAULT_HEARTBEAT_FILE_NAME};
use crate::lcm_integration::LcmIntegrationConfig;
use serde::{Deserialize, Serialize};
use sieve_runtime::RuntimeOrchestrator;
use sieve_types::{Capability, UncertainMode, UnknownMode};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const DEFAULT_POLICY_PATH: &str = "docs/policy/baseline-policy.toml";
const DEFAULT_SIEVE_DIR_NAME: &str = ".sieve";
static APPROVAL_ALLOWANCES_TMP_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct AppConfig {
    pub(crate) telegram_bot_token: String,
    pub(crate) telegram_chat_id: i64,
    pub(crate) telegram_poll_timeout_secs: u16,
    pub(crate) telegram_allowed_sender_user_ids: Option<BTreeSet<i64>>,
    pub(crate) sieve_home: PathBuf,
    pub(crate) policy_path: PathBuf,
    pub(crate) event_log_path: PathBuf,
    pub(crate) automation_store_path: PathBuf,
    pub(crate) codex_store_path: PathBuf,
    pub(crate) runtime_cwd: String,
    pub(crate) heartbeat_interval_ms: Option<u64>,
    pub(crate) heartbeat_prompt_override: Option<String>,
    pub(crate) heartbeat_file_path: PathBuf,
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) allowed_net_connect_scopes: Vec<String>,
    pub(crate) unknown_mode: UnknownMode,
    pub(crate) uncertain_mode: UncertainMode,
    pub(crate) max_concurrent_turns: usize,
    pub(crate) max_planner_steps: usize,
    pub(crate) max_summary_calls_per_turn: usize,
    pub(crate) lcm: LcmIntegrationConfig,
}

impl AppConfig {
    pub(crate) fn from_env() -> Result<Self, String> {
        let telegram_bot_token = required_env("TELEGRAM_BOT_TOKEN")?;
        let telegram_chat_id = required_env("TELEGRAM_CHAT_ID")?
            .parse::<i64>()
            .map_err(|err| format!("invalid TELEGRAM_CHAT_ID: {err}"))?;
        let telegram_poll_timeout_secs = parse_u16_env("SIEVE_TELEGRAM_POLL_TIMEOUT_SECS", 1)?;
        let telegram_allowed_sender_user_ids = parse_telegram_allowed_sender_user_ids(
            env::var("SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS").ok(),
        )?;
        let policy_path = parse_policy_path(env::var("SIEVE_POLICY_PATH").ok());
        let sieve_home = parse_sieve_home(env::var("SIEVE_HOME").ok(), env::var("HOME").ok());
        let event_log_path = runtime_event_log_path(&sieve_home);
        let runtime_cwd = env::var("SIEVE_RUNTIME_CWD").unwrap_or_else(|_| ".".to_string());
        let automation_store_path = automation_store_path(&sieve_home);
        let codex_store_path = codex_store_path(&sieve_home);
        let heartbeat_interval_ms = parse_optional_duration_env("SIEVE_HEARTBEAT_EVERY")?;
        let heartbeat_prompt_override = optional_non_empty_env("SIEVE_HEARTBEAT_PROMPT");
        let heartbeat_file_path = PathBuf::from(&runtime_cwd).join(DEFAULT_HEARTBEAT_FILE_NAME);
        let allowed_tools =
            parse_allowed_tools(&env::var("SIEVE_ALLOWED_TOOLS").unwrap_or_else(|_| {
                "bash,codex_exec,codex_session,endorse,declassify".to_string()
            }));
        if allowed_tools.is_empty() {
            return Err("SIEVE_ALLOWED_TOOLS must include at least one tool".to_string());
        }
        let max_concurrent_turns = parse_usize_env("SIEVE_MAX_CONCURRENT_TURNS", 4)?;
        if max_concurrent_turns == 0 {
            return Err("SIEVE_MAX_CONCURRENT_TURNS must be >= 1".to_string());
        }
        let max_planner_steps = parse_usize_env("SIEVE_MAX_PLANNER_STEPS", 3)?;
        if max_planner_steps == 0 {
            return Err("SIEVE_MAX_PLANNER_STEPS must be >= 1".to_string());
        }
        let max_summary_calls_per_turn = parse_usize_env("SIEVE_MAX_SUMMARY_CALLS_PER_TURN", 12)?;
        if max_summary_calls_per_turn == 0 {
            return Err("SIEVE_MAX_SUMMARY_CALLS_PER_TURN must be >= 1".to_string());
        }
        let lcm = LcmIntegrationConfig::from_sieve_home(&sieve_home);

        Ok(Self {
            telegram_bot_token,
            telegram_chat_id,
            telegram_poll_timeout_secs,
            telegram_allowed_sender_user_ids,
            sieve_home,
            policy_path,
            event_log_path,
            runtime_cwd,
            automation_store_path,
            codex_store_path,
            heartbeat_interval_ms,
            heartbeat_prompt_override,
            heartbeat_file_path,
            allowed_tools,
            allowed_net_connect_scopes: Vec::new(),
            unknown_mode: parse_unknown_mode(env::var("SIEVE_UNKNOWN_MODE").ok())?,
            uncertain_mode: parse_uncertain_mode(env::var("SIEVE_UNCERTAIN_MODE").ok())?,
            max_concurrent_turns,
            max_planner_steps,
            max_summary_calls_per_turn,
            lcm,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ApprovalAllowancesFile {
    schema_version: u16,
    allowances: Vec<Capability>,
}

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("missing required environment variable `{key}`"))
}

pub(crate) fn parse_policy_path(raw: Option<String>) -> PathBuf {
    match raw.map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(DEFAULT_POLICY_PATH),
    }
}

pub(crate) fn parse_sieve_home(
    raw_sieve_home: Option<String>,
    raw_home: Option<String>,
) -> PathBuf {
    match raw_sieve_home.map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => match raw_home.map(|value| value.trim().to_string()) {
            Some(value) if !value.is_empty() => PathBuf::from(value).join(DEFAULT_SIEVE_DIR_NAME),
            _ => PathBuf::from(DEFAULT_SIEVE_DIR_NAME),
        },
    }
}

pub(crate) fn runtime_event_log_path(sieve_home: &Path) -> PathBuf {
    sieve_home.join("logs/runtime-events.jsonl")
}

pub(crate) fn automation_store_path(sieve_home: &Path) -> PathBuf {
    sieve_home.join("state/automation.json")
}

pub(crate) fn codex_store_path(sieve_home: &Path) -> PathBuf {
    sieve_home.join("state/codex.db")
}

pub(crate) fn approval_allowances_path(sieve_home: &Path) -> PathBuf {
    sieve_home.join("state/approval-allowances.json")
}

pub(crate) fn load_approval_allowances(path: &Path) -> Result<Vec<Capability>, String> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("failed reading {}: {err}", path.display())),
    };
    let parsed: ApprovalAllowancesFile = serde_json::from_str(&body)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    if parsed.schema_version != 1 {
        return Err(format!(
            "unsupported approval allowances schema_version {} in {}",
            parsed.schema_version,
            path.display()
        ));
    }
    Ok(parsed.allowances)
}

pub(crate) fn save_approval_allowances(
    path: &Path,
    allowances: &[Capability],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating {}: {err}", parent.display()))?;
    }
    let payload = ApprovalAllowancesFile {
        schema_version: 1,
        allowances: allowances.to_vec(),
    };
    let encoded = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed encoding approval allowances: {err}"))?;
    let nonce = APPROVAL_ALLOWANCES_TMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), nonce));
    fs::write(&tmp_path, encoded)
        .map_err(|err| format!("failed writing {}: {err}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).map_err(|err| {
        format!(
            "failed renaming {} to {}: {err}",
            tmp_path.display(),
            path.display()
        )
    })
}

pub(crate) fn persist_runtime_approval_allowances(
    runtime: &RuntimeOrchestrator,
    sieve_home: &Path,
) -> Result<(), String> {
    let allowances = runtime
        .persistent_approval_allowances()
        .map_err(|err| format!("failed reading runtime approval allowances: {err}"))?;
    let path = approval_allowances_path(sieve_home);
    save_approval_allowances(&path, &allowances)
}

fn parse_u16_env(key: &str, default: u16) -> Result<u16, String> {
    match env::var(key) {
        Ok(raw) => raw
            .parse::<u16>()
            .map_err(|err| format!("invalid {key}: {err}")),
        Err(_) => Ok(default),
    }
}

fn parse_usize_env(key: &str, default: usize) -> Result<usize, String> {
    match env::var(key) {
        Ok(raw) => raw
            .parse::<usize>()
            .map_err(|err| format!("invalid {key}: {err}")),
        Err(_) => Ok(default),
    }
}

fn parse_optional_duration_env(key: &str) -> Result<Option<u64>, String> {
    match env::var(key) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                parse_duration_ms(trimmed)
                    .map(Some)
                    .map_err(|err| format!("invalid {key}: {err}"))
            }
        }
        Err(_) => Ok(None),
    }
}

fn optional_non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn parse_telegram_allowed_sender_user_ids(
    raw: Option<String>,
) -> Result<Option<BTreeSet<i64>>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut parsed = BTreeSet::new();
    for entry in trimmed
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        let id = entry.parse::<i64>().map_err(|err| {
            format!("invalid SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS entry `{entry}`: {err}")
        })?;
        parsed.insert(id);
    }

    if parsed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parsed))
    }
}

pub(crate) fn load_dotenv_if_present() -> Result<(), String> {
    load_dotenv_from_path(PathBuf::from(".env").as_path())
}

pub(crate) fn load_dotenv_from_path(path: &Path) -> Result<(), String> {
    match dotenvy::from_path(path) {
        Ok(()) => Ok(()),
        Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to load {}: {err}", path.display())),
    }
}

fn parse_allowed_tools(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for value in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if value.eq_ignore_ascii_case("brave_search") {
            continue;
        }
        let key = value.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(value.to_string());
        }
    }
    out
}

fn parse_unknown_mode(raw: Option<String>) -> Result<UnknownMode, String> {
    match raw
        .unwrap_or_else(|| "deny".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ask" => Ok(UnknownMode::Ask),
        "accept" => Ok(UnknownMode::Accept),
        "deny" => Ok(UnknownMode::Deny),
        other => Err(format!(
            "invalid SIEVE_UNKNOWN_MODE `{other}` (expected ask|accept|deny)"
        )),
    }
}

fn parse_uncertain_mode(raw: Option<String>) -> Result<UncertainMode, String> {
    match raw
        .unwrap_or_else(|| "deny".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ask" => Ok(UncertainMode::Ask),
        "accept" => Ok(UncertainMode::Accept),
        "deny" => Ok(UncertainMode::Deny),
        other => Err(format!(
            "invalid SIEVE_UNCERTAIN_MODE `{other}` (expected ask|accept|deny)"
        )),
    }
}
