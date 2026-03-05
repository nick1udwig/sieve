#![forbid(unsafe_code)]

mod lcm_integration;

use async_trait::async_trait;
use lcm_integration::{LcmIntegration, LcmIntegrationConfig};
use serde::{Deserialize, Serialize};
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge, TelegramPrompt,
};
use sieve_llm::{
    GuidanceModel, OpenAiGuidanceModel, OpenAiPlannerModel, OpenAiResponseModel,
    OpenAiSummaryModel, ResponseModel, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput,
    SummaryModel, SummaryRequest,
};
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{
    ApprovalBusError, EventLogError, InProcessApprovalBus, JsonlRuntimeEventLog, MainlineArtifact,
    MainlineArtifactKind, MainlineRunError, MainlineRunReport, MainlineRunRequest, MainlineRunner,
    PlannerRunRequest, PlannerRunResult, PlannerToolResult, RuntimeDeps, RuntimeDisposition,
    RuntimeEventLog, RuntimeOrchestrator, SystemClock as RuntimeClock,
};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{
    Action, ApprovalResolvedEvent, AssistantMessageEvent, Capability, Integrity,
    InteractionModality, ModalityContract, ModalityOverrideReason, PlannerGuidanceFrame,
    PlannerGuidanceInput, PlannerGuidanceSignal, Resource, RunId, RuntimeEvent, UncertainMode,
    UnknownMode,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc as tokio_mpsc, Semaphore};

#[derive(Clone)]
struct AppConfig {
    telegram_bot_token: String,
    telegram_chat_id: i64,
    telegram_poll_timeout_secs: u16,
    telegram_allowed_sender_user_ids: Option<BTreeSet<i64>>,
    sieve_home: PathBuf,
    policy_path: PathBuf,
    event_log_path: PathBuf,
    runtime_cwd: String,
    allowed_tools: Vec<String>,
    allowed_net_connect_scopes: Vec<String>,
    unknown_mode: UnknownMode,
    uncertain_mode: UncertainMode,
    max_concurrent_turns: usize,
    max_planner_steps: usize,
    max_summary_calls_per_turn: usize,
    lcm: LcmIntegrationConfig,
}

const DEFAULT_POLICY_PATH: &str = "docs/policy/baseline-policy.toml";
const DEFAULT_SIEVE_DIR_NAME: &str = ".sieve";
static APPROVAL_ALLOWANCES_TMP_NONCE: AtomicU64 = AtomicU64::new(1);

impl AppConfig {
    fn from_env() -> Result<Self, String> {
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
        let allowed_tools = parse_allowed_tools(
            &env::var("SIEVE_ALLOWED_TOOLS")
                .unwrap_or_else(|_| "bash,endorse,declassify".to_string()),
        );
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

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("missing required environment variable `{key}`"))
}

fn parse_policy_path(raw: Option<String>) -> PathBuf {
    match raw.map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(DEFAULT_POLICY_PATH),
    }
}

fn parse_sieve_home(raw_sieve_home: Option<String>, raw_home: Option<String>) -> PathBuf {
    match raw_sieve_home.map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => match raw_home.map(|value| value.trim().to_string()) {
            Some(value) if !value.is_empty() => PathBuf::from(value).join(DEFAULT_SIEVE_DIR_NAME),
            _ => PathBuf::from(DEFAULT_SIEVE_DIR_NAME),
        },
    }
}

fn runtime_event_log_path(sieve_home: &std::path::Path) -> PathBuf {
    sieve_home.join("logs/runtime-events.jsonl")
}

fn approval_allowances_path(sieve_home: &std::path::Path) -> PathBuf {
    sieve_home.join("state/approval-allowances.json")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ApprovalAllowancesFile {
    schema_version: u16,
    allowances: Vec<Capability>,
}

fn load_approval_allowances(path: &std::path::Path) -> Result<Vec<Capability>, String> {
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

fn save_approval_allowances(
    path: &std::path::Path,
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

fn persist_runtime_approval_allowances(
    runtime: &RuntimeOrchestrator,
    sieve_home: &std::path::Path,
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

fn parse_telegram_allowed_sender_user_ids(
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

fn load_dotenv_if_present() -> Result<(), String> {
    load_dotenv_from_path(PathBuf::from(".env").as_path())
}

fn load_dotenv_from_path(path: &std::path::Path) -> Result<(), String> {
    match dotenvy::from_path(path) {
        Ok(()) => Ok(()),
        Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("failed to load {}: {err}", path.display())),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConversationRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ConversationLogRecord {
    event: &'static str,
    schema_version: u16,
    run_id: RunId,
    role: ConversationRole,
    message: String,
    created_at_ms: u64,
}

impl ConversationLogRecord {
    fn new(run_id: RunId, role: ConversationRole, message: String, created_at_ms: u64) -> Self {
        Self {
            event: "conversation",
            schema_version: 1,
            run_id,
            role,
            message,
            created_at_ms,
        }
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

fn planner_allowed_tools_for_turn(
    configured_tools: &[String],
    has_known_value_refs: bool,
) -> Vec<String> {
    if has_known_value_refs {
        return configured_tools.to_vec();
    }

    configured_tools
        .iter()
        .filter(|tool| tool.as_str() != "endorse" && tool.as_str() != "declassify")
        .cloned()
        .collect()
}

fn planner_allowed_net_connect_scopes(policy: &TomlPolicyEngine) -> Vec<String> {
    let mut scopes = Vec::new();
    let mut seen = BTreeSet::new();
    for capability in &policy.config().allow_capabilities {
        if capability.resource != Resource::Net || capability.action != Action::Connect {
            continue;
        }
        let planner_scope = planner_net_connect_scope(&capability.scope);
        if seen.insert(planner_scope.clone()) {
            scopes.push(planner_scope);
        }
    }
    scopes
}

fn planner_net_connect_scope(scope: &str) -> String {
    let Ok(url) = reqwest::Url::parse(scope) else {
        return scope.to_string();
    };
    let Some(host) = url.host_str() else {
        return scope.to_string();
    };
    let mut origin = format!("{}://{}", url.scheme(), host.to_ascii_lowercase());
    if let Some(port) = url.port() {
        let default_port = match url.scheme() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        if Some(port) != default_port {
            origin.push(':');
            origin.push_str(&port.to_string());
        }
    }
    origin
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSource {
    Stdin,
    Telegram,
}

impl PromptSource {
    fn as_str(self) -> &'static str {
        match self {
            PromptSource::Stdin => "stdin",
            PromptSource::Telegram => "telegram",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IngressPrompt {
    source: PromptSource,
    text: String,
    modality: InteractionModality,
    media_file_id: Option<String>,
}

struct RuntimeBridge {
    approval_bus: Arc<InProcessApprovalBus>,
    prompt_tx: Option<tokio_mpsc::UnboundedSender<IngressPrompt>>,
}

impl RuntimeBridge {
    fn new(approval_bus: Arc<InProcessApprovalBus>) -> Self {
        Self {
            approval_bus,
            prompt_tx: None,
        }
    }

    fn with_prompt_tx(
        approval_bus: Arc<InProcessApprovalBus>,
        prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
    ) -> Self {
        Self {
            approval_bus,
            prompt_tx: Some(prompt_tx),
        }
    }
}

impl TelegramEventBridge for RuntimeBridge {
    fn publish_runtime_event(&self, _event: &RuntimeEvent) {}

    fn submit_approval(&self, approval: ApprovalResolvedEvent) {
        if let Err(err) = self.approval_bus.resolve(approval) {
            eprintln!(
                "telegram bridge failed to resolve approval: {}",
                format_approval_bus_error(&err)
            );
        }
    }

    fn submit_prompt(&self, prompt: TelegramPrompt) {
        let text = prompt.text.trim().to_string();
        if prompt.modality == InteractionModality::Text && text.is_empty() {
            return;
        }
        if let Some(prompt_tx) = &self.prompt_tx {
            if let Err(err) = prompt_tx.send(IngressPrompt {
                source: PromptSource::Telegram,
                text,
                modality: prompt.modality,
                media_file_id: prompt.media_file_id,
            }) {
                eprintln!("failed to enqueue telegram prompt: {err}");
            }
        }
    }
}

fn format_approval_bus_error(err: &ApprovalBusError) -> String {
    err.to_string()
}

struct AppMainlineRunner {
    artifact_root: PathBuf,
    next_artifact_id: AtomicU64,
}

impl AppMainlineRunner {
    fn new(artifact_root: PathBuf) -> Self {
        Self {
            artifact_root,
            next_artifact_id: AtomicU64::new(1),
        }
    }

    fn next_ref_id(&self) -> String {
        let next = self.next_artifact_id.fetch_add(1, Ordering::Relaxed);
        format!("artifact-{}-{next}", now_ms())
    }

    async fn persist_artifact(
        &self,
        run_id: &RunId,
        kind: MainlineArtifactKind,
        bytes: &[u8],
    ) -> Result<MainlineArtifact, MainlineRunError> {
        let ref_id = self.next_ref_id();
        let kind_name = match kind {
            MainlineArtifactKind::Stdout => "stdout",
            MainlineArtifactKind::Stderr => "stderr",
        };
        let run_dir = self.artifact_root.join(&run_id.0);
        tokio::fs::create_dir_all(&run_dir)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("create artifact dir failed: {err}")))?;
        let path = run_dir.join(format!("{ref_id}-{kind_name}.log"));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("persist artifact failed: {err}")))?;

        Ok(MainlineArtifact {
            ref_id,
            kind,
            path: path.to_string_lossy().to_string(),
            byte_count: bytes.len() as u64,
            line_count: count_newlines(bytes),
        })
    }
}

#[async_trait]
impl MainlineRunner for AppMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        let output = TokioCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .output()
            .await
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;

        let stdout_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stdout,
                &output.stdout,
            )
            .await?;
        let stderr_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stderr,
                &output.stderr,
            )
            .await?;

        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: output.status.code(),
            artifacts: vec![stdout_artifact, stderr_artifact],
        })
    }
}

fn count_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|byte| **byte == b'\n').count() as u64
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TelegramLoopEvent {
    Runtime(RuntimeEvent),
    TypingStart { run_id: String },
    TypingStop { run_id: String },
}

struct FanoutRuntimeEventLog {
    jsonl: JsonlRuntimeEventLog,
    history: Mutex<Vec<RuntimeEvent>>,
    telegram_tx: Mutex<Sender<TelegramLoopEvent>>,
}

impl FanoutRuntimeEventLog {
    fn new(
        path: impl Into<PathBuf>,
        telegram_tx: Sender<TelegramLoopEvent>,
    ) -> Result<Self, EventLogError> {
        Ok(Self {
            jsonl: JsonlRuntimeEventLog::new(path.into())?,
            history: Mutex::new(Vec::new()),
            telegram_tx: Mutex::new(telegram_tx),
        })
    }

    fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.history
            .lock()
            .expect("runtime event history lock poisoned")
            .clone()
    }

    async fn append_conversation(
        &self,
        record: ConversationLogRecord,
    ) -> Result<(), EventLogError> {
        let value =
            serde_json::to_value(record).map_err(|err| EventLogError::Append(err.to_string()))?;
        self.jsonl.append_json_value(&value).await
    }
}

#[async_trait]
impl RuntimeEventLog for FanoutRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        self.jsonl.append(event.clone()).await?;
        self.history
            .lock()
            .map_err(|_| EventLogError::Append("runtime event history lock poisoned".to_string()))?
            .push(event.clone());
        self.telegram_tx
            .lock()
            .map_err(|_| EventLogError::Append("telegram event sender lock poisoned".to_string()))?
            .send(TelegramLoopEvent::Runtime(event))
            .map_err(|err| {
                EventLogError::Append(format!("failed to forward runtime event: {err}"))
            })?;
        Ok(())
    }
}

fn spawn_telegram_loop(
    cfg: &AppConfig,
    bridge: RuntimeBridge,
    event_rx: Receiver<TelegramLoopEvent>,
) -> thread::JoinHandle<()> {
    let bot_token = cfg.telegram_bot_token.clone();
    let chat_id = cfg.telegram_chat_id;
    let poll_timeout_secs = cfg.telegram_poll_timeout_secs;
    let allowed_sender_user_ids = cfg.telegram_allowed_sender_user_ids.clone();

    thread::spawn(move || {
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id,
                poll_timeout_secs,
                allowed_sender_user_ids,
            },
            bridge,
            TelegramBotApiLongPoll::new(bot_token),
            TelegramClock,
        );

        loop {
            let mut disconnected = false;
            loop {
                match event_rx.try_recv() {
                    Ok(TelegramLoopEvent::Runtime(event)) => {
                        if let Err(err) = adapter.publish_runtime_event(event) {
                            eprintln!("telegram publish runtime event failed: {err:?}");
                        }
                    }
                    Ok(TelegramLoopEvent::TypingStart { run_id }) => {
                        if let Err(err) = adapter.start_typing(run_id) {
                            eprintln!("telegram typing start failed: {err:?}");
                        }
                    }
                    Ok(TelegramLoopEvent::TypingStop { run_id }) => {
                        adapter.stop_typing(&run_id);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if disconnected {
                break;
            }

            if let Err(err) = adapter.poll_once() {
                eprintln!("telegram poll failed: {err:?}");
                thread::sleep(Duration::from_secs(1));
            }
        }
    })
}

fn spawn_stdin_prompt_loop(
    prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    let prompt = line.trim();
                    if prompt.is_empty() {
                        continue;
                    }
                    if let Err(err) = prompt_tx.send(IngressPrompt {
                        source: PromptSource::Stdin,
                        text: prompt.to_string(),
                        modality: InteractionModality::Text,
                        media_file_id: None,
                    }) {
                        eprintln!("stdin prompt loop stopped: {err}");
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("stdin read failed: {err}");
                    break;
                }
            }
        }
    })
}

#[derive(Debug, Clone)]
enum RenderRef {
    Literal {
        value: String,
    },
    Artifact {
        path: PathBuf,
        byte_count: u64,
        line_count: u64,
    },
}

fn build_response_turn_input(
    run_id: &RunId,
    trusted_user_message: &str,
    planner_result: &PlannerRunResult,
) -> (ResponseTurnInput, BTreeMap<String, RenderRef>) {
    let mut render_refs = BTreeMap::new();
    let mut tool_outcomes = Vec::with_capacity(planner_result.tool_results.len());
    for tool_result in &planner_result.tool_results {
        tool_outcomes.push(summarize_tool_result(tool_result, &mut render_refs));
    }

    (
        ResponseTurnInput {
            run_id: run_id.clone(),
            trusted_user_message: trusted_user_message.to_string(),
            planner_thoughts: planner_result.thoughts.clone(),
            tool_outcomes,
        },
        render_refs,
    )
}

fn requires_output_visibility(input: &ResponseTurnInput) -> bool {
    !non_empty_output_ref_ids(input).is_empty()
        && user_explicitly_requests_output_visibility(&input.trusted_user_message)
}

fn user_explicitly_requests_output_visibility(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("output")
        || lower.contains("stdout")
        || lower.contains("stderr")
        || lower.contains("contents of")
        || lower.contains("content of")
        || lower.contains("show the result")
        || lower.contains("show me the result")
        || lower.contains("run exactly")
        || (lower.contains("what did") && lower.contains("return"))
}

fn output_ref_requires_visibility(kind: &str) -> bool {
    matches!(kind, "stdout" | "stderr")
}

fn non_empty_output_ref_ids(input: &ResponseTurnInput) -> BTreeSet<String> {
    input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|ref_metadata| {
            output_ref_requires_visibility(&ref_metadata.kind) && ref_metadata.byte_count > 0
        })
        .map(|ref_metadata| ref_metadata.ref_id.clone())
        .collect()
}

fn response_evidence_fingerprint(input: &ResponseTurnInput) -> String {
    let mut parts = Vec::new();
    for outcome in &input.tool_outcomes {
        parts.push(format!(
            "{}|{}|{}|{}",
            outcome.tool_name,
            outcome.outcome,
            outcome.attempted_command.as_deref().unwrap_or(""),
            outcome.failure_reason.as_deref().unwrap_or("")
        ));
        for metadata in &outcome.refs {
            parts.push(format!(
                "ref:{}:{}:{}:{}",
                metadata.ref_id, metadata.kind, metadata.byte_count, metadata.line_count
            ));
        }
    }
    parts.join("\n")
}

fn response_has_visible_selected_output(
    input: &ResponseTurnInput,
    response: &sieve_llm::ResponseTurnOutput,
) -> bool {
    let output_ref_ids = non_empty_output_ref_ids(input);
    response.referenced_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id) && response.message.contains(&format!("[[ref:{ref_id}]]"))
    }) || response.summarized_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id)
            && response.message.contains(&format!("[[summary:{ref_id}]]"))
    })
}

fn summarize_tool_result(
    result: &PlannerToolResult,
    render_refs: &mut BTreeMap<String, RenderRef>,
) -> ResponseToolOutcome {
    match result {
        PlannerToolResult::Bash {
            disposition,
            command,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: format!("executed mainline (exit_code={:?})", report.exit_code),
                attempted_command: Some(command.clone()),
                failure_reason: None,
                refs: report
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        render_refs.insert(
                            artifact.ref_id.clone(),
                            RenderRef::Artifact {
                                path: PathBuf::from(&artifact.path),
                                byte_count: artifact.byte_count,
                                line_count: artifact.line_count,
                            },
                        );
                        ResponseRefMetadata {
                            ref_id: artifact.ref_id.clone(),
                            kind: mainline_artifact_kind_name(artifact.kind).to_string(),
                            byte_count: artifact.byte_count,
                            line_count: artifact.line_count,
                        }
                    })
                    .collect(),
            },
            RuntimeDisposition::ExecuteQuarantine(report) => {
                let trace_ref = format!("trace:{}", report.run_id.0);
                render_refs.insert(
                    trace_ref.clone(),
                    RenderRef::Literal {
                        value: report.trace_path.clone(),
                    },
                );
                ResponseToolOutcome {
                    tool_name: "bash".to_string(),
                    outcome: format!(
                        "executed in quarantine (exit_code={:?}, trace=[[ref:{}]])",
                        report.exit_code, trace_ref
                    ),
                    attempted_command: Some(command.clone()),
                    failure_reason: None,
                    refs: vec![ResponseRefMetadata {
                        ref_id: trace_ref,
                        kind: "trace_path".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    }],
                }
            }
            RuntimeDisposition::Denied { reason } => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "denied".to_string(),
                attempted_command: Some(command.clone()),
                failure_reason: Some(reason.clone()),
                refs: Vec::new(),
            },
        },
        PlannerToolResult::Endorse {
            request,
            transition,
        } => {
            let value_ref_id = format!("value:{}", request.value_ref.0);
            render_refs.insert(
                value_ref_id.clone(),
                RenderRef::Literal {
                    value: request.value_ref.0.clone(),
                },
            );
            let outcome = match transition {
                Some(transition) => format!(
                    "endorse applied for [[ref:{}]] ({} -> {})",
                    value_ref_id,
                    format_integrity(transition.from_integrity),
                    format_integrity(transition.to_integrity),
                ),
                None => format!("endorse not applied for [[ref:{}]]", value_ref_id),
            };
            ResponseToolOutcome {
                tool_name: "endorse".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: value_ref_id,
                    kind: "value_ref".to_string(),
                    byte_count: 0,
                    line_count: 0,
                }],
            }
        }
        PlannerToolResult::Declassify {
            request,
            transition,
        } => {
            let value_ref_id = format!("value:{}", request.value_ref.0);
            let sink_ref_id = format!("sink:{}", request.sink.0);
            render_refs.insert(
                value_ref_id.clone(),
                RenderRef::Literal {
                    value: request.value_ref.0.clone(),
                },
            );
            render_refs.insert(
                sink_ref_id.clone(),
                RenderRef::Literal {
                    value: request.sink.0.clone(),
                },
            );
            let outcome = match transition {
                Some(transition) => format!(
                    "declassify applied for [[ref:{}]] -> [[ref:{}]] (already_allowed={})",
                    value_ref_id, sink_ref_id, transition.sink_was_already_allowed
                ),
                None => format!(
                    "declassify not applied for [[ref:{}]] -> [[ref:{}]]",
                    value_ref_id, sink_ref_id
                ),
            };
            ResponseToolOutcome {
                tool_name: "declassify".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: None,
                refs: vec![
                    ResponseRefMetadata {
                        ref_id: value_ref_id,
                        kind: "value_ref".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                    ResponseRefMetadata {
                        ref_id: sink_ref_id,
                        kind: "sink".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                ],
            }
        }
    }
}

fn mainline_artifact_kind_name(kind: MainlineArtifactKind) -> &'static str {
    match kind {
        MainlineArtifactKind::Stdout => "stdout",
        MainlineArtifactKind::Stderr => "stderr",
    }
}

fn format_integrity(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::Trusted => "trusted",
        Integrity::Untrusted => "untrusted",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BashActionClass {
    Discovery,
    Fetch,
    Extract,
    Other,
}

impl BashActionClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Fetch => "fetch",
            Self::Extract => "extract",
            Self::Other => "other",
        }
    }
}

const MIN_PRIMARY_FETCH_STDOUT_BYTES: u64 = 256;

#[derive(Debug, Clone, Copy, Default)]
struct ToolProgressSummary {
    discovery_success_count: usize,
    discovery_output_count: usize,
    fetch_success_count: usize,
    non_asset_fetch_output_count: usize,
    primary_fetch_output_count: usize,
    markdown_fetch_output_count: usize,
    denied_count: usize,
}

fn first_shell_word(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

fn classify_bash_action(command: &str) -> BashActionClass {
    let cmd = first_shell_word(command)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match cmd.as_str() {
        "bravesearch" | "brave-search" => BashActionClass::Discovery,
        "curl" | "wget" => BashActionClass::Fetch,
        "jq" | "awk" | "sed" | "grep" | "rg" => BashActionClass::Extract,
        _ => BashActionClass::Other,
    }
}

fn command_targets_markdown_view(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("https://markdown.new/") || lower.contains("http://markdown.new/")
}

fn command_targets_likely_asset(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("imgs.search.brave.com")
        || lower.contains("favicon")
        || lower.contains(".png")
        || lower.contains(".jpg")
        || lower.contains(".jpeg")
        || lower.contains(".gif")
        || lower.contains(".webp")
        || lower.contains(".svg")
        || lower.contains(".ico")
        || lower.contains(".css")
        || lower.contains(".js")
}

fn url_is_likely_asset(url: &str) -> bool {
    command_targets_likely_asset(url)
}

fn summarize_tool_progress(tool_results: &[PlannerToolResult]) -> ToolProgressSummary {
    let mut summary = ToolProgressSummary::default();
    for result in tool_results {
        match result {
            PlannerToolResult::Bash {
                command,
                disposition,
            } => {
                let action = classify_bash_action(command);
                match disposition {
                    RuntimeDisposition::ExecuteMainline(report) => {
                        let success = report.exit_code.unwrap_or(1) == 0;
                        let stdout_bytes: u64 = report
                            .artifacts
                            .iter()
                            .filter(|artifact| {
                                matches!(artifact.kind, MainlineArtifactKind::Stdout)
                            })
                            .map(|artifact| artifact.byte_count)
                            .sum();
                        let has_output = stdout_bytes > 0;
                        if success {
                            match action {
                                BashActionClass::Discovery => {
                                    summary.discovery_success_count =
                                        summary.discovery_success_count.saturating_add(1);
                                    if has_output {
                                        summary.discovery_output_count =
                                            summary.discovery_output_count.saturating_add(1);
                                    }
                                }
                                BashActionClass::Fetch => {
                                    summary.fetch_success_count =
                                        summary.fetch_success_count.saturating_add(1);
                                    if has_output && !command_targets_likely_asset(command) {
                                        summary.non_asset_fetch_output_count =
                                            summary.non_asset_fetch_output_count.saturating_add(1);
                                        if stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES {
                                            summary.primary_fetch_output_count = summary
                                                .primary_fetch_output_count
                                                .saturating_add(1);
                                        }
                                    }
                                    if has_output && command_targets_markdown_view(command) {
                                        summary.markdown_fetch_output_count =
                                            summary.markdown_fetch_output_count.saturating_add(1);
                                    }
                                }
                                BashActionClass::Extract | BashActionClass::Other => {}
                            }
                        }
                    }
                    RuntimeDisposition::Denied { .. } => {
                        summary.denied_count = summary.denied_count.saturating_add(1);
                    }
                    RuntimeDisposition::ExecuteQuarantine(_) => {}
                }
            }
            PlannerToolResult::Endorse { .. } | PlannerToolResult::Declassify { .. } => {}
        }
    }
    summary
}

fn summarize_observed_tool_result(result: &PlannerToolResult) -> serde_json::Value {
    match result {
        PlannerToolResult::Bash {
            command,
            disposition,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                let action_class = classify_bash_action(command);
                let stdout_bytes: u64 = report
                    .artifacts
                    .iter()
                    .filter(|artifact| matches!(artifact.kind, MainlineArtifactKind::Stdout))
                    .map(|artifact| artifact.byte_count)
                    .sum();
                let stderr_bytes: u64 = report
                    .artifacts
                    .iter()
                    .filter(|artifact| matches!(artifact.kind, MainlineArtifactKind::Stderr))
                    .map(|artifact| artifact.byte_count)
                    .sum();
                serde_json::json!({
                    "tool": "bash",
                    "command_len": command.len(),
                    "action_class": action_class.as_str(),
                    "disposition": "execute_mainline",
                    "exit_code": report.exit_code,
                    "artifact_count": report.artifacts.len(),
                    "stdout_bytes": stdout_bytes,
                    "stderr_bytes": stderr_bytes,
                    "likely_has_candidate_urls": matches!(action_class, BashActionClass::Discovery) && stdout_bytes > 0,
                    "likely_has_primary_content": matches!(action_class, BashActionClass::Fetch)
                        && stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES
                        && !command_targets_likely_asset(command),
                    "uses_markdown_view": command_targets_markdown_view(command),
                    "likely_asset_target": command_targets_likely_asset(command),
                })
            }
            RuntimeDisposition::ExecuteQuarantine(report) => serde_json::json!({
                "tool": "bash",
                "command_len": command.len(),
                "action_class": classify_bash_action(command).as_str(),
                "disposition": "execute_quarantine",
                "exit_code": report.exit_code,
                "trace_path_present": !report.trace_path.trim().is_empty(),
                "stdout_path_present": report.stdout_path.as_deref().is_some(),
                "stderr_path_present": report.stderr_path.as_deref().is_some()
            }),
            RuntimeDisposition::Denied { reason } => serde_json::json!({
                "tool": "bash",
                "command_len": command.len(),
                "action_class": classify_bash_action(command).as_str(),
                "disposition": "denied",
                "reason_len": reason.len()
            }),
        },
        PlannerToolResult::Endorse {
            request,
            transition,
        } => serde_json::json!({
            "tool": "endorse",
            "value_ref_len": request.value_ref.0.len(),
            "target_integrity": format_integrity(request.target_integrity),
            "applied": transition.is_some()
        }),
        PlannerToolResult::Declassify {
            request,
            transition,
        } => serde_json::json!({
            "tool": "declassify",
            "value_ref_len": request.value_ref.0.len(),
            "sink_len": request.sink.0.len(),
            "applied": transition.is_some()
        }),
    }
}

fn normalize_bash_command_for_repeat_guard(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn mainline_artifact_signature(report: &MainlineRunReport) -> Vec<(String, u64, u64)> {
    let mut signature = report
        .artifacts
        .iter()
        .map(|artifact| {
            (
                mainline_artifact_kind_name(artifact.kind).to_string(),
                artifact.byte_count,
                artifact.line_count,
            )
        })
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

fn has_repeated_bash_outcome(tool_results: &[PlannerToolResult]) -> bool {
    if tool_results.len() < 2 {
        return false;
    }

    let prev = &tool_results[tool_results.len() - 2];
    let last = &tool_results[tool_results.len() - 1];
    match (prev, last) {
        (
            PlannerToolResult::Bash {
                command: left_command,
                disposition: left_disposition,
            },
            PlannerToolResult::Bash {
                command: right_command,
                disposition: right_disposition,
            },
        ) if normalize_bash_command_for_repeat_guard(left_command)
            == normalize_bash_command_for_repeat_guard(right_command) =>
        {
            match (left_disposition, right_disposition) {
                (
                    RuntimeDisposition::ExecuteMainline(left_report),
                    RuntimeDisposition::ExecuteMainline(right_report),
                ) => {
                    left_report.exit_code == right_report.exit_code
                        && mainline_artifact_signature(left_report)
                            == mainline_artifact_signature(right_report)
                }
                (
                    RuntimeDisposition::Denied {
                        reason: left_reason,
                    },
                    RuntimeDisposition::Denied {
                        reason: right_reason,
                    },
                ) => left_reason == right_reason,
                _ => false,
            }
        }
        _ => false,
    }
}

fn build_guidance_prompt(
    trusted_user_message: &str,
    step_index: usize,
    max_steps: usize,
    step_results: &[PlannerToolResult],
    all_results: &[PlannerToolResult],
) -> String {
    let observed_results: Vec<serde_json::Value> = step_results
        .iter()
        .map(summarize_observed_tool_result)
        .collect();
    let step_progress = summarize_tool_progress(step_results);
    let total_progress = summarize_tool_progress(all_results);
    serde_json::json!({
        "task": "planner_act_observe",
        "trusted_user_message": trusted_user_message,
        "step_index": step_index,
        "max_steps": max_steps,
        "step_tool_result_count": step_results.len(),
        "total_tool_result_count": all_results.len(),
        "step_progress": {
            "discovery_success_count": step_progress.discovery_success_count,
            "discovery_output_count": step_progress.discovery_output_count,
            "fetch_success_count": step_progress.fetch_success_count,
            "non_asset_fetch_output_count": step_progress.non_asset_fetch_output_count,
            "primary_fetch_output_count": step_progress.primary_fetch_output_count,
            "markdown_fetch_output_count": step_progress.markdown_fetch_output_count,
            "denied_count": step_progress.denied_count,
        },
        "total_progress": {
            "discovery_success_count": total_progress.discovery_success_count,
            "discovery_output_count": total_progress.discovery_output_count,
            "fetch_success_count": total_progress.fetch_success_count,
            "non_asset_fetch_output_count": total_progress.non_asset_fetch_output_count,
            "primary_fetch_output_count": total_progress.primary_fetch_output_count,
            "markdown_fetch_output_count": total_progress.markdown_fetch_output_count,
            "denied_count": total_progress.denied_count,
            "has_repeated_no_gain": has_repeated_bash_outcome(all_results),
        },
        "observed_step_results": observed_results,
        "instruction": "Return numeric guidance code: continue only if more tool actions are still needed; otherwise return final or stop. When discovery output exists but non-asset fetch content is still missing, prefer continue code 110 before finalizing."
    })
    .to_string()
}

fn guidance_requests_continue(signal: PlannerGuidanceSignal) -> bool {
    matches!(
        signal,
        PlannerGuidanceSignal::ContinueNeedEvidence
            | PlannerGuidanceSignal::ContinueFetchPrimarySource
            | PlannerGuidanceSignal::ContinueFetchAdditionalSource
            | PlannerGuidanceSignal::ContinueRefineApproach
            | PlannerGuidanceSignal::ContinueNeedRequiredParameter
            | PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence
            | PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint
            | PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool
            | PlannerGuidanceSignal::ContinueNeedHigherQualitySource
            | PlannerGuidanceSignal::ContinueResolveSourceConflict
            | PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch
            | PlannerGuidanceSignal::ContinueNeedUrlExtraction
            | PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl
            | PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction
    )
}

fn guidance_continue_decision(
    signal: PlannerGuidanceSignal,
    consecutive_empty_steps: usize,
    planner_steps_taken: usize,
    planner_step_limit: usize,
    planner_step_hard_limit: usize,
) -> (bool, usize, bool) {
    let mut auto_extended_limit = false;
    let mut should_continue = guidance_requests_continue(signal) && consecutive_empty_steps < 2;
    let mut effective_step_limit = planner_step_limit;
    if should_continue && planner_steps_taken >= effective_step_limit {
        if effective_step_limit < planner_step_hard_limit {
            effective_step_limit = effective_step_limit.saturating_add(1);
            auto_extended_limit = true;
        } else {
            should_continue = false;
        }
    }
    (should_continue, effective_step_limit, auto_extended_limit)
}

fn signal_claims_fact_ready(signal: PlannerGuidanceSignal) -> bool {
    matches!(
        signal,
        PlannerGuidanceSignal::FinalAnswerReady
            | PlannerGuidanceSignal::FinalAnswerPartial
            | PlannerGuidanceSignal::FinalSingleFactReady
            | PlannerGuidanceSignal::FinalConflictingFactsWithRange
    )
}

fn signal_is_hard_stop(signal: PlannerGuidanceSignal) -> bool {
    matches!(
        signal,
        PlannerGuidanceSignal::StopPolicyBlocked
            | PlannerGuidanceSignal::StopBudgetExhausted
            | PlannerGuidanceSignal::StopNoAllowedToolCanSatisfyTask
            | PlannerGuidanceSignal::ErrorContractViolation
    )
}

fn progress_contract_override_signal(
    trusted_user_message: &str,
    signal: PlannerGuidanceSignal,
    tool_results: &[PlannerToolResult],
) -> Option<(PlannerGuidanceSignal, &'static str)> {
    if user_requested_sources(trusted_user_message) || signal_is_hard_stop(signal) {
        return None;
    }
    let progress = summarize_tool_progress(tool_results);
    if progress.discovery_output_count == 0 {
        return None;
    }
    if progress.primary_fetch_output_count > 0 {
        return None;
    }
    if progress.non_asset_fetch_output_count > 0 {
        if signal == PlannerGuidanceSignal::ContinueNeedHigherQualitySource {
            return None;
        }
        return Some((
            PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
            "fetch_output_low_signal",
        ));
    }
    if progress.fetch_success_count > 0 {
        if signal == PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl {
            return None;
        }
        return Some((
            PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl,
            "missing_non_asset_fetch_content",
        ));
    }
    if has_repeated_bash_outcome(tool_results) {
        if signal == PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction {
            return None;
        }
        return Some((
            PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction,
            "repeated_no_progress",
        ));
    }
    if signal == PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch {
        return None;
    }
    if !guidance_requests_continue(signal) && !signal_claims_fact_ready(signal) {
        return None;
    }
    Some((
        PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch,
        "missing_primary_content_fetch",
    ))
}

async fn render_assistant_message(
    message: &str,
    referenced_ref_ids: &BTreeSet<String>,
    summarized_ref_ids: &BTreeSet<String>,
    render_refs: &BTreeMap<String, RenderRef>,
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
) -> String {
    let mut expanded = message.to_string();

    for ref_id in referenced_ref_ids {
        if let Some(raw_value) = resolve_raw_ref_value(ref_id, render_refs).await {
            let token = format!("[[ref:{ref_id}]]");
            expanded = expanded.replace(&token, &raw_value);
        }
    }

    for ref_id in summarized_ref_ids {
        if let Some((content, byte_count, line_count)) =
            resolve_ref_summary_input(ref_id, render_refs).await
        {
            let summary = match summary_model
                .summarize_ref(SummaryRequest {
                    run_id: run_id.clone(),
                    ref_id: ref_id.clone(),
                    content,
                    byte_count,
                    line_count,
                })
                .await
            {
                Ok(summary) => summary,
                Err(err) => format!("summary unavailable: {err}"),
            };
            let token = format!("[[summary:{ref_id}]]");
            expanded = expanded.replace(&token, &summary);
        }
    }

    expanded
}

async fn resolve_raw_ref_value(
    ref_id: &str,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Option<String> {
    let render_ref = render_refs.get(ref_id)?;
    match render_ref {
        RenderRef::Literal { value } => Some(value.clone()),
        RenderRef::Artifact { path, .. } => read_artifact_as_string(path).await.ok(),
    }
}

async fn resolve_ref_summary_input(
    ref_id: &str,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Option<(String, u64, u64)> {
    let render_ref = render_refs.get(ref_id)?;
    match render_ref {
        RenderRef::Literal { value } => Some((value.clone(), value.len() as u64, 0)),
        RenderRef::Artifact {
            path,
            byte_count,
            line_count,
        } => {
            let content = read_artifact_as_string(path).await.ok()?;
            Some((content, *byte_count, *line_count))
        }
    }
}

fn dedupe_preserve_order(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn trim_url_candidate(candidate: &str) -> &str {
    let mut end = candidate.len();
    while end > 0 {
        let Some(ch) = candidate[..end].chars().next_back() else {
            break;
        };
        if matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\'' | '`'
        ) {
            end = end.saturating_sub(ch.len_utf8());
            continue;
        }
        break;
    }
    &candidate[..end]
}

fn extract_plain_urls_from_text(message: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    while cursor < message.len() {
        let remaining = &message[cursor..];
        let http_pos = remaining.find("http://");
        let https_pos = remaining.find("https://");
        let Some(rel_start) = (match (http_pos, https_pos) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }) else {
            break;
        };
        let start = cursor + rel_start;
        let mut end = message.len();
        for (offset, ch) in message[start..].char_indices() {
            if offset == 0 {
                continue;
            }
            if ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '\\' | '`') {
                end = start + offset;
                break;
            }
        }
        let candidate = trim_url_candidate(&message[start..end]);
        if candidate.starts_with("https://") || candidate.starts_with("http://") {
            urls.push(candidate.to_string());
        }
        cursor = end.max(start.saturating_add(1));
    }
    dedupe_preserve_order(urls)
}

fn filter_non_asset_urls(urls: Vec<String>) -> Vec<String> {
    dedupe_preserve_order(
        urls.into_iter()
            .filter(|url| !url_is_likely_asset(url))
            .collect(),
    )
}

fn strip_asset_urls_from_message(message: &str) -> String {
    let mut sanitized = message.to_string();
    for url in extract_plain_urls_from_text(message) {
        if url_is_likely_asset(&url) {
            sanitized = sanitized.replace(&url, "");
        }
    }
    let mut lines = Vec::new();
    for line in sanitized.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    lines.join("\n")
}

fn contains_plain_url(message: &str) -> bool {
    message.contains("https://") || message.contains("http://")
}

fn mentions_linkish_text(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains(" link")
        || normalized.contains(" links")
        || normalized.contains("url")
        || normalized.contains("source")
        || normalized.contains("full results")
        || normalized.contains("search results")
}

fn split_sentences(message: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in message.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
            }
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn remove_linkish_sentences(message: &str) -> String {
    let kept: Vec<String> = split_sentences(message)
        .into_iter()
        .filter(|sentence| {
            let lower = sentence.to_ascii_lowercase();
            !lower.contains(" link")
                && !lower.contains(" links")
                && !lower.contains("url")
                && !lower.contains("source")
                && !lower.contains("full results")
                && !lower.contains("search results")
        })
        .collect();
    if kept.is_empty() {
        message
            .replace("provided link", "")
            .replace("provided links", "")
            .trim()
            .to_string()
    } else {
        kept.join(" ")
    }
}

fn enforce_link_policy(
    message: String,
    source_urls: &[String],
    trusted_user_message: &str,
) -> String {
    if !mentions_linkish_text(&message) || contains_plain_url(&message) {
        return message;
    }
    if !source_urls.is_empty() && user_requested_sources(trusted_user_message) {
        let mut out = message.trim().to_string();
        if !out.is_empty() {
            out.push('\n');
        }
        for url in source_urls.iter().take(3) {
            out.push_str(url);
            out.push('\n');
        }
        return out.trim().to_string();
    }
    remove_linkish_sentences(&message)
}

fn normalized_words(input: &str) -> String {
    input
        .to_ascii_lowercase()
        .replace(
            ['?', '!', '.', ',', ';', ':', '(', ')', '[', ']', '{', '}'],
            " ",
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn user_requested_sources(trusted_user_message: &str) -> bool {
    let normalized = normalized_words(trusted_user_message);
    normalized.contains("source")
        || normalized.contains("sources")
        || normalized.contains("link")
        || normalized.contains("links")
        || normalized.contains("url")
        || normalized.contains("citation")
        || normalized.contains("citations")
        || normalized.contains("reference")
        || normalized.contains("references")
}

fn user_requested_detailed_output(trusted_user_message: &str) -> bool {
    let normalized = normalized_words(trusted_user_message);
    normalized.contains("detailed")
        || normalized.contains("in detail")
        || normalized.contains("step by step")
        || normalized.contains("full breakdown")
        || normalized.contains("thorough")
        || normalized.contains("comprehensive")
        || normalized.contains("long form")
        || normalized.contains("explain")
}

fn sentence_like_count(message: &str) -> usize {
    split_sentences(message)
        .into_iter()
        .filter(|sentence| !sentence.trim().is_empty())
        .count()
}

fn concise_style_diagnostic(composed_message: &str, trusted_user_message: &str) -> Option<String> {
    if user_requested_detailed_output(trusted_user_message) {
        return None;
    }
    let sentence_count = sentence_like_count(composed_message);
    let char_count = composed_message.chars().count();
    if sentence_count > 4 || char_count > 650 {
        return Some(
            "response is too long; keep to 1-2 concise sentences unless user asks for detail"
                .to_string(),
        );
    }
    let url_count = extract_plain_urls_from_text(composed_message).len();
    if url_count > 1 && !user_requested_sources(trusted_user_message) {
        return Some(
            "response includes unsolicited source dump; keep at most one URL unless user asks for sources"
                .to_string(),
        );
    }
    None
}

fn obvious_meta_compose_pattern(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    let starts_with_meta = normalized.starts_with("the assistant ")
        || normalized.starts_with("assistant is ")
        || normalized.starts_with("user asks")
        || normalized.starts_with("the user asks")
        || normalized.starts_with("quality gate")
        || normalized.starts_with("quality gate outcome")
        || normalized.starts_with("grounding gate")
        || normalized.starts_with("evidence summary")
        || normalized.starts_with("the evidence summary")
        || normalized.starts_with("draft reply");
    let contains_meta = normalized.contains("the user has")
        || normalized.contains("user has asked")
        || normalized.contains("diagnostic notes")
        || normalized.contains("draft reply says")
        || normalized.contains("the assistant is ready to help")
        || normalized.contains("quality gate")
        || normalized.contains("grounding gate")
        || normalized.contains("evidence summary")
        || normalized.contains("no relevant evidence was found")
        || normalized.contains("unsupported claim")
        || normalized.contains("ungrounded");
    starts_with_meta || contains_meta
}

fn compact_single_line(input: &str, max_len: usize) -> String {
    let compact = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_len {
        return compact;
    }
    let mut out = String::new();
    for ch in compact.chars().take(max_len.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

fn denied_outcomes_only_message(response_input: &ResponseTurnInput) -> Option<String> {
    if response_input.tool_outcomes.is_empty() {
        return None;
    }

    let all_denied = response_input
        .tool_outcomes
        .iter()
        .all(|outcome| outcome.failure_reason.is_some());
    if !all_denied {
        return None;
    }

    let mut seen = BTreeSet::new();
    let mut details = Vec::new();
    for outcome in &response_input.tool_outcomes {
        let reason = match outcome.failure_reason.as_deref() {
            Some(value) => compact_single_line(value, 120),
            None => continue,
        };
        let command = compact_single_line(
            outcome
                .attempted_command
                .as_deref()
                .unwrap_or(&outcome.tool_name),
            140,
        );
        if seen.insert((command.clone(), reason.clone())) {
            details.push((command, reason));
        }
    }

    if details.is_empty() {
        return None;
    }

    let mut message = details
        .iter()
        .take(2)
        .map(|(command, reason)| format!("I tried `{command}`, but it was blocked: {reason}."))
        .collect::<Vec<_>>()
        .join(" ");
    if details.len() > 2 {
        message.push_str(" I hit the same restriction on additional attempts.");
    }
    message.push_str(" I can try a different command path if you want.");
    Some(message)
}

fn strip_unexpanded_render_tokens(message: &str) -> String {
    let mut remaining = message;
    let mut out = String::new();
    loop {
        let Some(start) = remaining.find("[[") else {
            out.push_str(remaining);
            break;
        };
        out.push_str(&remaining[..start]);
        let tail = &remaining[start..];
        if tail.starts_with("[[ref:") || tail.starts_with("[[summary:") {
            if let Some(end) = tail.find("]]") {
                remaining = &tail[end + 2..];
                continue;
            }
        }
        out.push_str("[[");
        remaining = &tail[2..];
    }
    out.trim().to_string()
}

fn missing_connect_sink_from_reason(reason: &str) -> Option<&str> {
    reason
        .trim()
        .strip_prefix("missing capability Net:Connect:")
        .map(str::trim)
        .filter(|sink| !sink.is_empty())
}

fn markdown_wrapped_raw_url(command: &str) -> Option<String> {
    extract_plain_urls_from_text(command)
        .into_iter()
        .find_map(|url| {
            url.strip_prefix("https://markdown.new/")
                .or_else(|| url.strip_prefix("http://markdown.new/"))
                .map(str::trim)
                .map(str::to_string)
        })
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
}

fn low_signal_markdown_fetch_candidates(
    tool_results: &[PlannerToolResult],
) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for result in tool_results.iter().rev().take(8) {
        let PlannerToolResult::Bash {
            command,
            disposition: RuntimeDisposition::ExecuteMainline(report),
        } = result
        else {
            continue;
        };
        if classify_bash_action(command) != BashActionClass::Fetch
            || !command_targets_markdown_view(command)
        {
            continue;
        }
        let stdout_bytes: u64 = report
            .artifacts
            .iter()
            .filter(|artifact| matches!(artifact.kind, MainlineArtifactKind::Stdout))
            .map(|artifact| artifact.byte_count)
            .sum();
        if stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES {
            continue;
        }
        let Some(raw_url) = markdown_wrapped_raw_url(command) else {
            continue;
        };
        if seen.insert(raw_url.clone()) {
            candidates.push((command.clone(), raw_url));
        }
    }
    candidates.reverse();
    candidates
}

fn planner_policy_feedback(tool_results: &[PlannerToolResult]) -> Option<String> {
    let mut denied_sinks = Vec::new();
    let mut seen = BTreeSet::new();
    for result in tool_results.iter().rev().take(8) {
        let PlannerToolResult::Bash {
            command,
            disposition: RuntimeDisposition::Denied { reason },
        } = result
        else {
            continue;
        };
        let Some(sink) = missing_connect_sink_from_reason(reason) else {
            continue;
        };
        if seen.insert(sink.to_string()) {
            denied_sinks.push((sink.to_string(), command.clone()));
        }
    }
    let markdown_fallbacks = low_signal_markdown_fetch_candidates(tool_results);
    if denied_sinks.is_empty() && markdown_fallbacks.is_empty() {
        return None;
    }

    denied_sinks.reverse();
    let mut lines = Vec::new();
    if !denied_sinks.is_empty() {
        lines.push(
            "Policy feedback (trusted): recent network targets were denied for missing connect capability."
                .to_string(),
        );
        for (sink, command) in denied_sinks.iter().take(2) {
            lines.push(format!("- denied sink: {sink}"));
            lines.push(format!("- denied command: {command}"));
        }
        lines.push(
            "Do not repeat the same denied command; choose a different allowed action path."
                .to_string(),
        );
    }
    if let Some((_, raw_url)) = markdown_fallbacks.first() {
        lines.push(
            "Trusted fetch feedback: markdown proxy fetch returned low/no usable primary content."
                .to_string(),
        );
        lines.push(format!(
            "- fallback next fetch to raw URL once: curl -sS \"{raw_url}\""
        ));
        lines.push(
            "If direct fetch is denied by policy, switch to a different allowed source URL."
                .to_string(),
        );
    }
    lines.push(
        "For webpage fetches with `curl`, prefer `https://markdown.new/<url>` first; if it fails to yield usable content, try the raw URL once before repeating markdown.new."
            .to_string(),
    );
    Some(lines.join("\n"))
}

fn is_sieve_lcm_query_command(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    matches!(
        (parts.next(), parts.next()),
        (Some("sieve-lcm-cli"), Some("query"))
    )
}

fn trim_for_prompt(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = String::new();
    for ch in trimmed.chars().take(max_chars.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

async fn planner_memory_feedback(tool_results: &[PlannerToolResult]) -> Option<String> {
    for result in tool_results.iter().rev().take(8) {
        let PlannerToolResult::Bash {
            command,
            disposition: RuntimeDisposition::ExecuteMainline(report),
        } = result
        else {
            continue;
        };
        if report.exit_code.unwrap_or(1) != 0 || !is_sieve_lcm_query_command(command) {
            continue;
        }
        let stdout_artifact = report.artifacts.iter().find(|artifact| {
            matches!(artifact.kind, MainlineArtifactKind::Stdout) && artifact.byte_count > 0
        })?;
        let stdout = read_artifact_as_string(Path::new(&stdout_artifact.path))
            .await
            .ok()?;
        let payload: serde_json::Value = serde_json::from_str(&stdout).ok()?;

        let trusted_excerpts = payload
            .get("trusted_hits")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("excerpt").and_then(serde_json::Value::as_str))
                    .map(|value| trim_for_prompt(value, 220))
                    .filter(|value| !value.is_empty())
                    .take(3)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let untrusted_refs = payload
            .get("untrusted_refs")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("ref").and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .take(5)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if trusted_excerpts.is_empty() && untrusted_refs.is_empty() {
            continue;
        }

        let mut lines = Vec::new();
        lines.push(
            "Memory query feedback (trusted): use trusted excerpts below as evidence; untrusted refs are opaque."
                .to_string(),
        );
        for excerpt in trusted_excerpts {
            lines.push(format!("- trusted excerpt: {excerpt}"));
        }
        for reference in untrusted_refs {
            lines.push(format!("- untrusted ref: {reference}"));
        }
        return Some(lines.join("\n"));
    }
    None
}

async fn append_jsonl_record(path: &Path, value: &serde_json::Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("jsonl path has no parent: {}", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|err| format!("failed to create jsonl parent dir: {err}"))?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|err| format!("failed to open jsonl log `{}`: {err}", path.display()))?;
    let line = format!("{value}\n");
    file.write_all(line.as_bytes())
        .await
        .map_err(|err| format!("failed to append jsonl log `{}`: {err}", path.display()))
}

async fn append_turn_controller_event(
    sieve_home: &Path,
    run_id: &RunId,
    phase: &str,
    payload: serde_json::Value,
) {
    let path = sieve_home.join("logs/turn-controller-events.jsonl");
    let record = serde_json::json!({
        "event": "turn_controller",
        "schema_version": 1,
        "created_at_ms": now_ms(),
        "run_id": run_id.0,
        "phase": phase,
        "payload": payload,
    });
    if let Err(err) = append_jsonl_record(&path, &record).await {
        eprintln!(
            "turn controller log write failed for {} (phase={}): {}",
            run_id.0, phase, err
        );
    }
}

async fn summarize_with_ref_id(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    ref_id: &str,
    payload: &serde_json::Value,
) -> Option<String> {
    let content = payload.to_string();
    let request = SummaryRequest {
        run_id: run_id.clone(),
        ref_id: ref_id.to_string(),
        byte_count: content.len() as u64,
        line_count: count_newlines(content.as_bytes()),
        content,
    };
    match summary_model.summarize_ref(request).await {
        Ok(summary) => {
            let trimmed = summary.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(_) => None,
    }
}

async fn summarize_with_ref_id_counted(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    ref_id: &str,
    payload: &serde_json::Value,
    summary_calls: &mut usize,
    budget_remaining: usize,
) -> Option<String> {
    if *summary_calls >= budget_remaining {
        return None;
    }
    *summary_calls = summary_calls.saturating_add(1);
    summarize_with_ref_id(summary_model, run_id, ref_id, payload).await
}

fn extract_trusted_evidence_lines(
    trusted_user_message: &str,
    planner_thoughts: Option<&str>,
) -> Vec<String> {
    let mut lines = vec![format!("[user] {trusted_user_message}")];
    if let Some(thoughts) = planner_thoughts {
        for line in thoughts.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("[user] ") {
                lines.push(trimmed.to_string());
            }
        }
    }
    dedupe_preserve_order(lines)
}

#[cfg(test)]
fn compose_quality_requires_retry(
    composed_message: &str,
    quality_gate: Option<&str>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    match parse_gate_verdict(quality_gate) {
        None | Some(GateVerdict::Pass) => None,
        Some(GateVerdict::Revise(reason)) => Some(reason),
    }
}

#[cfg(test)]
fn gate_requires_retry(gate: Option<&str>) -> Option<String> {
    match parse_gate_verdict(gate) {
        None | Some(GateVerdict::Pass) => None,
        Some(GateVerdict::Revise(reason)) => Some(reason),
    }
}

fn combine_gate_reasons(gates: &[Option<String>]) -> Option<String> {
    let mut combined = Vec::new();
    for gate in gates {
        if let Some(gate) = gate.as_deref() {
            let trimmed = gate.trim();
            if !trimmed.is_empty() {
                combined.push(trimmed.to_string());
            }
        }
    }
    if combined.is_empty() {
        None
    } else {
        Some(combined.join(" | "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposePlannerDecision {
    Finalize,
    Continue(PlannerGuidanceSignal),
}

struct ComposeAssistantOutcome {
    message: String,
    quality_gate: Option<String>,
    planner_decision: ComposePlannerDecision,
    summary_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateVerdict {
    Pass,
    Revise(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ComposeGateOutput {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    continue_code: Option<u16>,
}

fn parse_gate_verdict(gate: Option<&str>) -> Option<GateVerdict> {
    let gate = gate.unwrap_or("").trim();
    if gate.is_empty() {
        return None;
    }
    let lower = gate.to_ascii_lowercase();
    if let Some(revise_idx) = lower.find("revise") {
        let reason = gate[revise_idx + "revise".len()..]
            .trim_start_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
            .trim();
        if reason.is_empty() {
            return Some(GateVerdict::Revise("requested revision".to_string()));
        }
        return Some(GateVerdict::Revise(reason.to_string()));
    }
    if lower.starts_with("pass") || (lower.contains("pass") && !lower.contains("revise")) {
        return Some(GateVerdict::Pass);
    }
    Some(GateVerdict::Revise(format!(
        "unstructured gate output: {}",
        compact_single_line(gate, 200)
    )))
}

fn followup_signal_from_reason(
    reason: &str,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let has_tool_context = response_input
        .tool_outcomes
        .iter()
        .any(|outcome| !outcome.refs.is_empty() || outcome.failure_reason.is_some());
    if !has_tool_context {
        return None;
    }

    let lower = reason.to_ascii_lowercase();
    let is_style_only = lower.contains("third-person")
        || lower.contains("third person")
        || lower.contains("meta narration")
        || lower.contains("tone");
    if is_style_only {
        return None;
    }

    let denied_command_present = response_input.tool_outcomes.iter().any(|outcome| {
        outcome
            .failure_reason
            .as_deref()
            .map(|reason| {
                let reason = reason.to_ascii_lowercase();
                reason.contains("denied")
                    || reason.contains("blocked")
                    || reason.contains("not allowed")
                    || reason.contains("unknown command")
            })
            .unwrap_or(false)
    });
    if denied_command_present
        || lower.contains("denied")
        || lower.contains("blocked")
        || lower.contains("not allowed")
        || lower.contains("unknown command")
        || lower.contains("tool failure")
    {
        return Some(PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool);
    }

    if lower.contains("conflict")
        || lower.contains("contradict")
        || lower.contains("inconsistent")
        || lower.contains("disagree")
    {
        return Some(PlannerGuidanceSignal::ContinueResolveSourceConflict);
    }

    if lower.contains("stale")
        || lower.contains("outdated")
        || lower.contains("latest")
        || lower.contains("fresh")
        || lower.contains("current as of")
        || lower.contains("time-bound")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence);
    }

    if lower.contains("no progress")
        || lower.contains("repeated")
        || lower.contains("same command")
        || lower.contains("no evidence gain")
    {
        return Some(PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction);
    }

    if lower.contains("asset")
        || lower.contains("favicon")
        || lower.contains("image url")
        || lower.contains("non-content url")
        || lower.contains("canonical content page")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl);
    }

    if lower.contains("extract url")
        || lower.contains("url extraction")
        || lower.contains("parse urls")
        || lower.contains("extract links")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedUrlExtraction);
    }

    if lower.contains("primary source")
        || lower.contains("primary-page")
        || lower.contains("primary content")
        || lower.contains("discovery/search snippets")
        || lower.contains("snippet-only")
        || lower.contains("insufficient source")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch);
    }

    if lower.contains("higher quality")
        || lower.contains("low quality source")
        || lower.contains("needs citation")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedHigherQualitySource);
    }

    if lower.contains("missing parameter")
        || lower.contains("need user input")
        || lower.contains("needs clarification")
        || lower.contains("please specify")
        || lower.contains("missing required")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedRequiredParameter);
    }

    if lower.contains("preference")
        || lower.contains("constraint")
        || lower.contains("format")
        || lower.contains("units")
        || lower.contains("locale")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint);
    }

    // Fallback stays generic to avoid domain-specific keyword routing.
    Some(PlannerGuidanceSignal::ContinueRefineApproach)
}

#[cfg(test)]
fn compose_quality_followup_signal(
    quality_gate: Option<&str>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let reason = match parse_gate_verdict(quality_gate) {
        None | Some(GateVerdict::Pass) => return None,
        Some(GateVerdict::Revise(reason)) => reason,
    };
    followup_signal_from_reason(&reason, response_input)
}

fn continue_signal_from_code(code: u16) -> Option<PlannerGuidanceSignal> {
    PlannerGuidanceSignal::try_from(code)
        .ok()
        .filter(|signal| guidance_requests_continue(*signal))
}

fn parse_compose_gate_output(raw: Option<&str>) -> Option<ComposeGateOutput> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<ComposeGateOutput>(raw) {
        let verdict = parsed.verdict.trim().to_ascii_uppercase();
        let reason = parsed.reason.map(|value| value.trim().to_string());
        if verdict == "PASS" {
            return Some(ComposeGateOutput {
                verdict,
                reason: None,
                continue_code: parsed
                    .continue_code
                    .and_then(continue_signal_from_code)
                    .map(|s| s.code()),
            });
        }
        return Some(ComposeGateOutput {
            verdict: "REVISE".to_string(),
            reason: reason
                .filter(|value| !value.is_empty())
                .or_else(|| Some("requested revision".to_string())),
            continue_code: parsed
                .continue_code
                .and_then(continue_signal_from_code)
                .map(|signal| signal.code()),
        });
    }
    match parse_gate_verdict(Some(raw)) {
        None => None,
        Some(GateVerdict::Pass) => Some(ComposeGateOutput {
            verdict: "PASS".to_string(),
            reason: None,
            continue_code: None,
        }),
        Some(GateVerdict::Revise(reason)) => Some(ComposeGateOutput {
            verdict: "REVISE".to_string(),
            reason: Some(reason),
            continue_code: None,
        }),
    }
}

fn compose_gate_requires_retry(
    composed_message: &str,
    trusted_user_message: &str,
    gate: Option<&ComposeGateOutput>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    if let Some(diagnostic) = concise_style_diagnostic(composed_message, trusted_user_message) {
        return Some(diagnostic);
    }
    let gate = gate?;
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    gate.reason
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some("requested revision".to_string()))
}

fn compose_gate_followup_signal(
    gate: Option<&ComposeGateOutput>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let gate = gate?;
    if let Some(signal) = gate.continue_code.and_then(continue_signal_from_code) {
        return Some(signal);
    }
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    let reason = gate.reason.as_deref().unwrap_or("requested revision");
    followup_signal_from_reason(reason, response_input)
}

async fn write_compose_audit_artifacts(
    sieve_home: &Path,
    run_id: &RunId,
    attempts: &[serde_json::Value],
    final_message: &str,
    output_ref_ids: &[String],
    source_urls: &[String],
    quality_gate: Option<&str>,
    grounding_gate: Option<&str>,
    planner_followup_signal: Option<PlannerGuidanceSignal>,
) -> Result<(), String> {
    let run_dir = sieve_home.join("artifacts").join(&run_id.0);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|err| format!("failed to create compose artifact dir: {err}"))?;

    let mut input_refs = Vec::new();
    for (idx, attempt) in attempts.iter().enumerate() {
        let ref_id = format!("assistant-compose-input:{}:{}", run_id.0, idx + 1);
        let path = run_dir.join(format!("assistant-compose-input-{}.json", idx + 1));
        let content = serde_json::to_vec_pretty(attempt)
            .map_err(|err| format!("failed to encode compose payload: {err}"))?;
        tokio::fs::write(&path, content)
            .await
            .map_err(|err| format!("failed to write compose payload artifact: {err}"))?;
        input_refs.push(serde_json::json!({
            "ref_id": ref_id,
            "path": path.to_string_lossy(),
        }));
    }

    let output_ref_id = format!("assistant-compose-output:{}", run_id.0);
    let output_path = run_dir.join("assistant-compose-output.txt");
    tokio::fs::write(&output_path, final_message.as_bytes())
        .await
        .map_err(|err| format!("failed to write compose output artifact: {err}"))?;

    let logs_path = sieve_home.join("logs/compose-events.jsonl");
    let record = serde_json::json!({
        "schema_version": 1,
        "event": "compose_audit",
        "created_at_ms": now_ms(),
        "run_id": run_id.0,
        "input_refs": input_refs,
        "output_ref": {
            "ref_id": output_ref_id,
            "path": output_path.to_string_lossy(),
        },
        "output_ref_ids": output_ref_ids,
        "source_urls": source_urls,
        "quality_gate": quality_gate,
        "grounding_gate": grounding_gate,
        "planner_followup_signal_code": planner_followup_signal.map(PlannerGuidanceSignal::code),
    });
    append_jsonl_record(&logs_path, &record).await
}

async fn collect_source_urls_from_refs(
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut seen = BTreeSet::new();
    for outcome in &response_input.tool_outcomes {
        for metadata in &outcome.refs {
            if metadata.byte_count == 0 {
                continue;
            }
            let Some((content, _, _)) =
                resolve_ref_summary_input(&metadata.ref_id, render_refs).await
            else {
                continue;
            };
            for url in extract_plain_urls_from_text(&content) {
                if seen.insert(url.clone()) {
                    urls.push(url);
                }
                if urls.len() >= 8 {
                    return urls;
                }
            }
        }
    }
    urls
}

async fn build_compose_evidence_summaries(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut seen = BTreeSet::new();
    for (idx, metadata) in response_input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|metadata| metadata.byte_count > 0)
        .enumerate()
    {
        if idx >= 4 {
            break;
        }
        if !seen.insert(metadata.ref_id.clone()) {
            continue;
        }
        let Some((content, _, _)) = resolve_ref_summary_input(&metadata.ref_id, render_refs).await
        else {
            continue;
        };
        let cache_key = format!(
            "{}:{}:{}:{}",
            trusted_user_message, metadata.ref_id, metadata.byte_count, metadata.line_count
        );
        if let Some(summary) = evidence_cache.get(&cache_key) {
            if !summary.trim().is_empty() {
                summaries.push(summary.clone());
            }
            continue;
        }
        let payload = serde_json::json!({
            "task": "compose_evidence_extract",
            "trusted_user_message": trusted_user_message,
            "ref_id": metadata.ref_id,
            "content": content,
        });
        let ref_id = format!("assistant-compose-evidence:{}:{}", run_id.0, idx + 1);
        if let Some(summary) = summarize_with_ref_id_counted(
            summary_model,
            run_id,
            &ref_id,
            &payload,
            summary_calls,
            summary_budget,
        )
        .await
        {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                summaries.push(trimmed.to_string());
                evidence_cache.insert(cache_key, trimmed.to_string());
            }
        }
    }
    summaries
}

async fn run_compose_gate(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    trusted_evidence: &[String],
    composed_message: &str,
    evidence_summaries: &[String],
    source_urls: &[String],
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Option<ComposeGateOutput> {
    let payload = serde_json::json!({
        "task": "compose_gate",
        "trusted_user_message": trusted_user_message,
        "user_requested_sources": user_requested_sources(trusted_user_message),
        "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
        "trusted_evidence": trusted_evidence,
        "composed_message": composed_message,
        "evidence_summaries": evidence_summaries,
        "source_urls": source_urls,
    });
    let raw = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-compose-gate:{}", run_id.0),
        &payload,
        summary_calls,
        summary_budget,
    )
    .await;
    parse_compose_gate_output(raw.as_deref())
}

async fn compose_assistant_message(
    summary_model: &dyn SummaryModel,
    sieve_home: &Path,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    draft_message: String,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_budget: usize,
) -> ComposeAssistantOutcome {
    let mut summary_calls = 0usize;
    let output_ref_ids: Vec<String> = non_empty_output_ref_ids(response_input)
        .into_iter()
        .collect();
    let mut source_urls = dedupe_preserve_order(extract_plain_urls_from_text(&draft_message));
    source_urls.extend(collect_source_urls_from_refs(response_input, render_refs).await);
    source_urls = filter_non_asset_urls(dedupe_preserve_order(source_urls));
    let trusted_evidence = extract_trusted_evidence_lines(
        trusted_user_message,
        response_input.planner_thoughts.as_deref(),
    );
    let evidence_summaries = build_compose_evidence_summaries(
        summary_model,
        run_id,
        trusted_user_message,
        response_input,
        render_refs,
        evidence_cache,
        &mut summary_calls,
        summary_budget,
    )
    .await;
    let tool_outcomes: Vec<serde_json::Value> = response_input
        .tool_outcomes
        .iter()
        .map(|outcome| {
            serde_json::json!({
                "tool_name": outcome.tool_name,
                "outcome": outcome.outcome,
                "attempted_command": outcome.attempted_command,
                "failure_reason": outcome.failure_reason,
                "refs": outcome.refs.iter().map(|ref_metadata| {
                    serde_json::json!({
                        "ref_id": ref_metadata.ref_id,
                        "kind": ref_metadata.kind,
                        "byte_count": ref_metadata.byte_count,
                        "line_count": ref_metadata.line_count,
                    })
                }).collect::<Vec<_>>()
            })
        })
        .collect();

    let mut attempt_payloads = Vec::new();
    let payload = serde_json::json!({
        "task": "compose_user_reply",
        "trusted_user_message": trusted_user_message,
        "user_requested_sources": user_requested_sources(trusted_user_message),
        "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
        "trusted_evidence": trusted_evidence.clone(),
        "assistant_draft_message": draft_message,
        "planner_thoughts": response_input.planner_thoughts.clone(),
        "tool_outcomes": tool_outcomes,
        "output_ref_ids": output_ref_ids.clone(),
        "available_plain_urls": source_urls.clone(),
        "evidence_summaries": evidence_summaries.clone(),
    });
    attempt_payloads.push(payload.clone());

    let first_composed = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-compose:{}", run_id.0),
        &payload,
        &mut summary_calls,
        summary_budget,
    )
    .await
    .unwrap_or_else(|| {
        payload
            .get("assistant_draft_message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });

    let mut composed = first_composed;
    let mut gate = run_compose_gate(
        summary_model,
        run_id,
        trusted_user_message,
        &trusted_evidence,
        &composed,
        &evidence_summaries,
        &source_urls,
        &mut summary_calls,
        summary_budget,
    )
    .await;
    let mut retry_diagnostics = Vec::new();
    if let Some(diagnostic) =
        compose_gate_requires_retry(&composed, trusted_user_message, gate.as_ref())
    {
        retry_diagnostics.push(diagnostic);
    }
    let did_retry = !retry_diagnostics.is_empty() && summary_calls < summary_budget;
    if did_retry {
        let retry_diagnostic = retry_diagnostics.join(" | ");
        let retry_payload = serde_json::json!({
            "task": "compose_user_reply",
            "trusted_user_message": trusted_user_message,
            "user_requested_sources": user_requested_sources(trusted_user_message),
            "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
            "trusted_evidence": trusted_evidence.clone(),
            "assistant_draft_message": payload
                .get("assistant_draft_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            "planner_thoughts": response_input.planner_thoughts.clone(),
            "tool_outcomes": payload
                .get("tool_outcomes")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            "output_ref_ids": output_ref_ids.clone(),
            "available_plain_urls": source_urls.clone(),
            "evidence_summaries": evidence_summaries.clone(),
            "compose_diagnostic": retry_diagnostic,
            "previous_composed_message": composed,
        });
        attempt_payloads.push(retry_payload.clone());
        composed = summarize_with_ref_id_counted(
            summary_model,
            run_id,
            &format!("assistant-compose-retry:{}", run_id.0),
            &retry_payload,
            &mut summary_calls,
            summary_budget,
        )
        .await
        .unwrap_or_else(|| {
            retry_payload
                .get("previous_composed_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        gate = run_compose_gate(
            summary_model,
            run_id,
            trusted_user_message,
            &trusted_evidence,
            &composed,
            &evidence_summaries,
            &source_urls,
            &mut summary_calls,
            summary_budget,
        )
        .await;
    }

    let quality_gate = match gate.as_ref() {
        Some(value) if value.verdict.eq_ignore_ascii_case("PASS") => Some("PASS".to_string()),
        Some(value) => Some(format!(
            "REVISE: {}",
            value
                .reason
                .as_deref()
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or("requested revision")
        )),
        None if summary_calls >= summary_budget => {
            Some("REVISE: summary call budget exhausted".to_string())
        }
        None => Some("REVISE: missing gate verdict".to_string()),
    };
    let grounding_gate: Option<String> = None;
    let combined_gate = combine_gate_reasons(&[quality_gate.clone()]);
    let planner_followup_signal = if summary_calls >= summary_budget {
        None
    } else {
        compose_gate_followup_signal(gate.as_ref(), response_input)
    };
    let planner_decision = planner_followup_signal
        .map(ComposePlannerDecision::Continue)
        .unwrap_or(ComposePlannerDecision::Finalize);

    let mut composed = enforce_link_policy(composed, &source_urls, trusted_user_message);
    composed = strip_asset_urls_from_message(&composed);
    if let Some(message) = denied_outcomes_only_message(response_input) {
        composed = message;
    }
    if obvious_meta_compose_pattern(&composed) {
        if let Some(message) = denied_outcomes_only_message(response_input) {
            composed = message;
        } else {
            let draft_fallback = payload
                .get("assistant_draft_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !draft_fallback.is_empty() && !obvious_meta_compose_pattern(&draft_fallback) {
                composed = draft_fallback;
            }
        }
    }
    composed = strip_asset_urls_from_message(&composed);
    composed = strip_unexpanded_render_tokens(&composed);
    if let Err(err) = write_compose_audit_artifacts(
        sieve_home,
        run_id,
        &attempt_payloads,
        &composed,
        &output_ref_ids,
        &source_urls,
        quality_gate.as_deref(),
        grounding_gate.as_deref(),
        planner_followup_signal,
    )
    .await
    {
        eprintln!("compose audit write failed for {}: {}", run_id.0, err);
    }
    ComposeAssistantOutcome {
        message: composed,
        quality_gate: combined_gate,
        planner_decision,
        summary_calls,
    }
}

async fn read_artifact_as_string(path: &std::path::Path) -> Result<String, io::Error> {
    let bytes = tokio::fs::read(path).await?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn default_modality_contract(input: InteractionModality) -> ModalityContract {
    ModalityContract {
        input,
        response: input,
        override_reason: None,
    }
}

fn override_modality_contract(
    contract: &mut ModalityContract,
    response: InteractionModality,
    reason: ModalityOverrideReason,
) {
    contract.response = response;
    contract.override_reason = Some(reason);
}

fn command_error_from_output(context: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{context} failed")
    } else {
        format!("{context} failed: {stderr}")
    }
}

const CODEX_IMAGE_OCR_PROMPT: &str =
    "Extract the user's request and any relevant visible text from this image. Return plain text only.";
const ST_TTS_OUTPUT_FORMAT: &str = "ogg";

fn codex_image_ocr_args(input_path: &Path) -> Vec<std::ffi::OsString> {
    vec![
        "exec".into(),
        "--sandbox".into(),
        "read-only".into(),
        "--ephemeral".into(),
        "--image".into(),
        input_path.as_os_str().to_owned(),
        "--".into(),
        CODEX_IMAGE_OCR_PROMPT.into(),
    ]
}

fn st_audio_stt_args(input_path: &Path) -> Vec<std::ffi::OsString> {
    vec!["stt".into(), input_path.as_os_str().to_owned()]
}

fn st_audio_tts_args(text_path: &Path, output_path: &Path) -> Vec<std::ffi::OsString> {
    vec![
        "tts".into(),
        text_path.as_os_str().to_owned(),
        "--format".into(),
        ST_TTS_OUTPUT_FORMAT.into(),
        "--output".into(),
        output_path.as_os_str().to_owned(),
    ]
}

async fn fetch_telegram_file_path(bot_token: &str, file_id: &str) -> Result<String, String> {
    let url = format!("https://api.telegram.org/bot{bot_token}/getFile");
    let output = TokioCommand::new("curl")
        .arg("-sS")
        .arg("--fail")
        .arg("--get")
        .arg("--data-urlencode")
        .arg(format!("file_id={file_id}"))
        .arg(url)
        .output()
        .await
        .map_err(|err| format!("failed to fetch telegram file metadata: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("telegram getFile", &output));
    }
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("invalid telegram getFile response: {err}"))?;
    payload
        .pointer("/result/file_path")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| "telegram getFile response missing result.file_path".to_string())
}

async fn download_telegram_file(
    bot_token: &str,
    file_path: &str,
    destination: &std::path::Path,
) -> Result<(), String> {
    let url = format!("https://api.telegram.org/file/bot{bot_token}/{file_path}");
    let output = TokioCommand::new("curl")
        .arg("-sS")
        .arg("--fail")
        .arg("-o")
        .arg(destination)
        .arg(url)
        .output()
        .await
        .map_err(|err| format!("failed to download telegram file: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("telegram file download", &output));
    }
    Ok(())
}

async fn transcribe_audio_prompt(
    cfg: &AppConfig,
    run_id: &RunId,
    file_id: &str,
) -> Result<String, String> {
    let file_path = fetch_telegram_file_path(&cfg.telegram_bot_token, file_id).await?;
    let media_dir = cfg.sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let ext = std::path::Path::new(&file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .unwrap_or("ogg");
    let input_path = media_dir.join(format!("voice-input.{ext}"));
    download_telegram_file(&cfg.telegram_bot_token, &file_path, &input_path).await?;

    let mut command = TokioCommand::new("st");
    for arg in st_audio_stt_args(&input_path) {
        command.arg(arg);
    }
    let output = command
        .output()
        .await
        .map_err(|err| format!("audio STT command spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("audio STT command", &output));
    }
    let transcript = String::from_utf8_lossy(&output.stdout).to_string();
    let transcript = transcript.trim().to_string();
    if transcript.is_empty() {
        return Err("audio STT command produced empty transcript".to_string());
    }
    Ok(transcript)
}

async fn extract_image_prompt(
    bot_token: &str,
    sieve_home: &Path,
    run_id: &RunId,
    file_id: &str,
) -> Result<String, String> {
    let file_path = fetch_telegram_file_path(bot_token, file_id).await?;
    let media_dir = sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let ext = std::path::Path::new(&file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .unwrap_or("jpg");
    let input_path = media_dir.join(format!("image-input.{ext}"));
    download_telegram_file(bot_token, &file_path, &input_path).await?;

    let mut command = TokioCommand::new("codex");
    for arg in codex_image_ocr_args(&input_path) {
        command.arg(arg);
    }
    let output = command
        .output()
        .await
        .map_err(|err| format!("image OCR command spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("image OCR command", &output));
    }
    let extracted = String::from_utf8_lossy(&output.stdout).to_string();
    let extracted = extracted.trim().to_string();
    if extracted.is_empty() {
        return Err("image OCR command produced empty output".to_string());
    }
    Ok(extracted)
}

async fn synthesize_audio_reply(
    cfg: &AppConfig,
    run_id: &RunId,
    assistant_message: &str,
) -> Result<PathBuf, String> {
    let media_dir = cfg.sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let text_path = media_dir.join("tts-input.txt");
    let output_path = media_dir.join("tts-output.ogg");
    tokio::fs::write(&text_path, assistant_message)
        .await
        .map_err(|err| format!("failed to write TTS input text: {err}"))?;

    let mut command = TokioCommand::new("st");
    for arg in st_audio_tts_args(&text_path, &output_path) {
        command.arg(arg);
    }
    let output = command
        .output()
        .await
        .map_err(|err| format!("audio TTS command spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("audio TTS command", &output));
    }

    let metadata = tokio::fs::metadata(&output_path)
        .await
        .map_err(|err| format!("audio TTS output missing: {err}"))?;
    if metadata.len() == 0 {
        return Err("audio TTS output file is empty".to_string());
    }
    Ok(output_path)
}

async fn send_telegram_voice(
    bot_token: &str,
    chat_id: i64,
    audio_path: &std::path::Path,
) -> Result<(), String> {
    let endpoint = format!("https://api.telegram.org/bot{bot_token}/sendVoice");
    let voice_arg = format!("voice=@{}", audio_path.to_string_lossy());
    let output = TokioCommand::new("curl")
        .arg("-sS")
        .arg("--fail")
        .arg("-X")
        .arg("POST")
        .arg("-F")
        .arg(format!("chat_id={chat_id}"))
        .arg("-F")
        .arg(voice_arg)
        .arg(endpoint)
        .output()
        .await
        .map_err(|err| format!("failed to send telegram voice message: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output("telegram sendVoice", &output));
    }
    Ok(())
}

async fn emit_assistant_error_message(
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    error_message: String,
) -> Result<(), EventLogError> {
    event_log
        .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: run_id.clone(),
            message: error_message.clone(),
            created_at_ms: now_ms(),
        }))
        .await?;
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            error_message,
            now_ms(),
        ))
        .await
}

async fn run_turn(
    runtime: &RuntimeOrchestrator,
    guidance_model: &dyn GuidanceModel,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_index: u64,
    source: PromptSource,
    input_modality: InteractionModality,
    media_file_id: Option<String>,
    user_message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_id = RunId(format!("run-{run_index}"));
    let mut modality_contract = default_modality_contract(input_modality);
    if modality_contract.response == InteractionModality::Image {
        override_modality_contract(
            &mut modality_contract,
            InteractionModality::Text,
            ModalityOverrideReason::NotSupported,
        );
    }
    let (trusted_user_message, input_error) = match input_modality {
        InteractionModality::Text => (user_message.clone(), None),
        InteractionModality::Audio => match media_file_id.as_deref() {
            Some(file_id) => match transcribe_audio_prompt(cfg, &run_id, file_id).await {
                Ok(transcript) => (transcript, None),
                Err(err) => (
                    String::new(),
                    Some(format!("audio input unavailable: {err}")),
                ),
            },
            None => (
                String::new(),
                Some("audio input missing media file id".to_string()),
            ),
        },
        InteractionModality::Image => match media_file_id.as_deref() {
            Some(file_id) => {
                match extract_image_prompt(
                    &cfg.telegram_bot_token,
                    &cfg.sieve_home,
                    &run_id,
                    file_id,
                )
                .await
                {
                    Ok(extracted) => (extracted, None),
                    Err(err) => (
                        String::new(),
                        Some(format!("image input unavailable: {err}")),
                    ),
                }
            }
            None => (
                String::new(),
                Some("image input missing media file id".to_string()),
            ),
        },
    };
    if let Some(error_message) = input_error {
        println!("{}: {}", run_id.0, error_message);
        emit_assistant_error_message(event_log, &run_id, error_message).await?;
        return Ok(());
    }

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_user_message(&trusted_user_message).await {
            eprintln!("lcm ingest user failed for {}: {}", run_id.0, err);
        }
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::User,
            trusted_user_message.clone(),
            now_ms(),
        ))
        .await?;

    let mut aggregated_result = PlannerRunResult {
        thoughts: None,
        tool_results: Vec::new(),
    };
    let mut planner_guidance: Option<PlannerGuidanceFrame> = None;
    let mut consecutive_empty_steps = 0usize;
    let mut planner_steps_taken = 0usize;
    let mut compose_followup_cycles = 0usize;
    let mut summary_calls_used = 0usize;
    let mut compose_continue_fingerprints = BTreeSet::new();
    let mut compose_evidence_cache = BTreeMap::new();
    let max_compose_followup_cycles = cfg.max_planner_steps.max(1);
    let planner_step_hard_limit = cfg
        .max_planner_steps
        .saturating_add(max_compose_followup_cycles);
    let mut planner_step_limit = cfg.max_planner_steps.max(1);
    let planner_user_message = trusted_user_message.clone();

    let assistant_message = loop {
        while planner_steps_taken < planner_step_limit {
            let step_number = planner_steps_taken + 1;
            let policy_feedback = planner_policy_feedback(&aggregated_result.tool_results);
            let memory_feedback = planner_memory_feedback(&aggregated_result.tool_results).await;
            let planner_turn_user_message = match (policy_feedback, memory_feedback) {
                (Some(policy), Some(memory)) => {
                    format!("{planner_user_message}\n\n{policy}\n\n{memory}")
                }
                (Some(policy), None) => format!("{planner_user_message}\n\n{policy}"),
                (None, Some(memory)) => format!("{planner_user_message}\n\n{memory}"),
                (None, None) => planner_user_message.clone(),
            };
            let has_known_value_refs = runtime.has_known_value_refs()?;
            let allowed_tools_for_turn =
                planner_allowed_tools_for_turn(&cfg.allowed_tools, has_known_value_refs);
            let step_result = match runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: run_id.clone(),
                    cwd: cfg.runtime_cwd.clone(),
                    user_message: planner_turn_user_message,
                    allowed_tools: allowed_tools_for_turn,
                    allowed_net_connect_scopes: cfg.allowed_net_connect_scopes.clone(),
                    previous_events: event_log.snapshot(),
                    guidance: planner_guidance.clone(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: cfg.unknown_mode,
                    uncertain_mode: cfg.uncertain_mode,
                })
                .await
            {
                Ok(result) => result,
                Err(err) => {
                    if let Err(log_err) =
                        emit_assistant_error_message(event_log, &run_id, format!("error: {err}"))
                            .await
                    {
                        eprintln!("failed to append assistant error conversation log: {log_err}");
                    }
                    return Err(err.into());
                }
            };

            planner_steps_taken = planner_steps_taken.saturating_add(1);
            let step_tool_count = step_result.tool_results.len();
            if step_tool_count == 0 {
                consecutive_empty_steps = consecutive_empty_steps.saturating_add(1);
            } else {
                consecutive_empty_steps = 0;
            }
            if let Some(thoughts) = step_result.thoughts.clone() {
                aggregated_result.thoughts = Some(thoughts);
            }
            let step_results = step_result.tool_results;
            aggregated_result.tool_results.extend(step_results.clone());
            if let Err(err) = persist_runtime_approval_allowances(runtime, &cfg.sieve_home) {
                eprintln!(
                    "failed to persist approval allowances for {}: {}",
                    run_id.0, err
                );
            }

            if has_repeated_bash_outcome(&aggregated_result.tool_results) {
                let can_retry =
                    planner_steps_taken < planner_step_limit && consecutive_empty_steps < 2;
                append_turn_controller_event(
                        &cfg.sieve_home,
                        &run_id,
                        "planner_repeat_guard",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                            "reason": "detected repeated bash command/result; forcing action-change guidance",
                            "continue": can_retry,
                            "next_signal_code": PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                        }),
                    )
                    .await;
                if can_retry {
                    planner_guidance = Some(PlannerGuidanceFrame {
                        code: PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                        confidence_bps: 9_000,
                        source_hit_index: None,
                        evidence_ref_index: None,
                    });
                    continue;
                }
                break;
            }

            let guidance_prompt = build_guidance_prompt(
                &trusted_user_message,
                step_number,
                cfg.max_planner_steps,
                &step_results,
                &aggregated_result.tool_results,
            );
            let guidance_output = match guidance_model
                .classify_guidance(PlannerGuidanceInput {
                    run_id: run_id.clone(),
                    prompt: guidance_prompt,
                })
                .await
            {
                Ok(output) => output,
                Err(err) => {
                    eprintln!(
                        "guidance model failed for {} at step {}: {}",
                        run_id.0, step_number, err
                    );
                    append_turn_controller_event(
                        &cfg.sieve_home,
                        &run_id,
                        "planner_guidance_error",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                        }),
                    )
                    .await;
                    break;
                }
            };
            let signal = match guidance_output.guidance.signal() {
                Ok(signal) => signal,
                Err(err) => {
                    eprintln!(
                        "invalid guidance signal for {} at step {}: {}",
                        run_id.0, step_number, err
                    );
                    append_turn_controller_event(
                        &cfg.sieve_home,
                        &run_id,
                        "planner_guidance_invalid",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                        }),
                    )
                    .await;
                    break;
                }
            };
            let override_applied = progress_contract_override_signal(
                &trusted_user_message,
                signal,
                &aggregated_result.tool_results,
            );
            let effective_signal = override_applied
                .map(|(override_signal, _)| override_signal)
                .unwrap_or(signal);
            let (should_continue, next_step_limit, auto_extended_limit) =
                guidance_continue_decision(
                    effective_signal,
                    consecutive_empty_steps,
                    planner_steps_taken,
                    planner_step_limit,
                    planner_step_hard_limit,
                );
            planner_step_limit = next_step_limit;
            append_turn_controller_event(
                &cfg.sieve_home,
                &run_id,
                "planner_guidance",
                serde_json::json!({
                    "step_number": step_number,
                    "signal_code": signal.code(),
                    "effective_signal_code": effective_signal.code(),
                    "override_reason": override_applied.map(|(_, reason)| reason),
                    "continue": should_continue,
                    "step_tool_count": step_tool_count,
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "auto_extended_limit": auto_extended_limit,
                    "consecutive_empty_steps": consecutive_empty_steps,
                }),
            )
            .await;
            let mut guidance_frame = guidance_output.guidance;
            guidance_frame.code = effective_signal.code();
            planner_guidance = Some(guidance_frame);
            if !should_continue {
                break;
            }
        }

        let (response_input, render_refs) =
            build_response_turn_input(&run_id, &trusted_user_message, &aggregated_result);
        let mut response_input = response_input;
        let mut response_output = match response_model
            .write_turn_response(response_input.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(log_err) =
                    emit_assistant_error_message(event_log, &run_id, format!("error: {err}")).await
                {
                    eprintln!("failed to append assistant error conversation log: {log_err}");
                }
                return Err(err.into());
            }
        };

        if requires_output_visibility(&response_input)
            && !response_has_visible_selected_output(&response_input, &response_output)
        {
            // One regeneration pass: enforce that non-empty output refs are either shown raw
            // or summarized by Q-LLM, without exposing untrusted strings to the model.
            let diagnostics = "Non-empty output refs exist (stdout/stderr). Include at least one output token directly in `message` using [[ref:<id>]] or [[summary:<id>]], and list the same id in referenced_ref_ids or summarized_ref_ids.";
            response_input.planner_thoughts = Some(match response_input.planner_thoughts.take() {
                Some(existing) if !existing.trim().is_empty() => {
                    format!("{existing}\n{diagnostics}")
                }
                _ => diagnostics.to_string(),
            });

            response_output = match response_model
                .write_turn_response(response_input.clone())
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    if let Err(log_err) =
                        emit_assistant_error_message(event_log, &run_id, format!("error: {err}"))
                            .await
                    {
                        eprintln!("failed to append assistant error conversation log: {log_err}");
                    }
                    return Err(err.into());
                }
            };

            if !response_has_visible_selected_output(&response_input, &response_output) {
                if let Some(fallback_ref_id) =
                    non_empty_output_ref_ids(&response_input).into_iter().next()
                {
                    response_output
                        .summarized_ref_ids
                        .insert(fallback_ref_id.clone());
                    let token = format!("[[summary:{fallback_ref_id}]]");
                    if !response_output.message.contains(&token) {
                        let base = response_output.message.trim();
                        response_output.message = if base.is_empty() {
                            token
                        } else {
                            format!("{base}\n{token}")
                        };
                    }
                }
            }
        }

        let output_visibility_required = requires_output_visibility(&response_input);
        let evidence_fingerprint = response_evidence_fingerprint(&response_input);
        let draft_message = if output_visibility_required {
            render_assistant_message(
                &response_output.message,
                &response_output.referenced_ref_ids,
                &response_output.summarized_ref_ids,
                &render_refs,
                summary_model,
                &run_id,
            )
            .await
        } else {
            let stripped = strip_unexpanded_render_tokens(&response_output.message);
            if stripped.trim().is_empty() {
                render_assistant_message(
                    &response_output.message,
                    &response_output.referenced_ref_ids,
                    &response_output.summarized_ref_ids,
                    &render_refs,
                    summary_model,
                    &run_id,
                )
                .await
            } else {
                stripped
            }
        };
        let remaining_summary_budget = cfg
            .max_summary_calls_per_turn
            .saturating_sub(summary_calls_used);
        let composed = if remaining_summary_budget == 0 {
            ComposeAssistantOutcome {
                message: draft_message,
                quality_gate: Some("REVISE: summary call budget exhausted".to_string()),
                planner_decision: ComposePlannerDecision::Finalize,
                summary_calls: 0,
            }
        } else {
            compose_assistant_message(
                summary_model,
                &cfg.sieve_home,
                &run_id,
                &trusted_user_message,
                &response_input,
                &render_refs,
                draft_message,
                &mut compose_evidence_cache,
                remaining_summary_budget,
            )
            .await
        };
        summary_calls_used = summary_calls_used.saturating_add(composed.summary_calls);

        if let ComposePlannerDecision::Continue(signal) = composed.planner_decision {
            let mut can_continue = planner_steps_taken < planner_step_hard_limit
                && compose_followup_cycles < max_compose_followup_cycles;
            let mut continue_block_reason: Option<&str> = None;
            if can_continue && summary_calls_used >= cfg.max_summary_calls_per_turn {
                can_continue = false;
                continue_block_reason = Some("summary_budget_exhausted");
            }
            if can_continue && !compose_continue_fingerprints.insert(evidence_fingerprint.clone()) {
                can_continue = false;
                continue_block_reason = Some("no_new_evidence");
            }
            append_turn_controller_event(
                &cfg.sieve_home,
                &run_id,
                "compose_decision",
                serde_json::json!({
                    "planner_decision_code": signal.code(),
                    "quality_gate_len": composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "compose_followup_cycles": compose_followup_cycles,
                    "continue": can_continue,
                    "continue_block_reason": continue_block_reason,
                    "summary_calls_used": summary_calls_used,
                    "summary_call_budget": cfg.max_summary_calls_per_turn,
                }),
            )
            .await;
            if can_continue {
                compose_followup_cycles = compose_followup_cycles.saturating_add(1);
                planner_step_limit = planner_step_limit
                    .saturating_add(1)
                    .min(planner_step_hard_limit.max(1));
                planner_guidance = Some(PlannerGuidanceFrame {
                    code: signal.code(),
                    confidence_bps: 9_000,
                    source_hit_index: None,
                    evidence_ref_index: None,
                });
                continue;
            }
        }

        append_turn_controller_event(
            &cfg.sieve_home,
            &run_id,
            "turn_finalize",
            serde_json::json!({
                "planner_steps_taken": planner_steps_taken,
                "planner_step_limit": planner_step_limit,
                "planner_step_hard_limit": planner_step_hard_limit,
                "compose_followup_cycles": compose_followup_cycles,
                "quality_gate_len": composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                "summary_calls_used": summary_calls_used,
                "summary_call_budget": cfg.max_summary_calls_per_turn,
            }),
        )
        .await;
        break composed.message;
    };
    println!("{}: {}", run_id.0, assistant_message);

    let mut delivered_audio = false;
    if source == PromptSource::Telegram && modality_contract.response == InteractionModality::Audio
    {
        match synthesize_audio_reply(cfg, &run_id, &assistant_message).await {
            Ok(audio_path) => {
                if let Err(err) =
                    send_telegram_voice(&cfg.telegram_bot_token, cfg.telegram_chat_id, &audio_path)
                        .await
                {
                    eprintln!("audio reply delivery failed for {}: {}", run_id.0, err);
                    override_modality_contract(
                        &mut modality_contract,
                        InteractionModality::Text,
                        ModalityOverrideReason::ToolFailure,
                    );
                } else {
                    delivered_audio = true;
                }
            }
            Err(err) => {
                eprintln!("audio synthesis failed for {}: {}", run_id.0, err);
                override_modality_contract(
                    &mut modality_contract,
                    InteractionModality::Text,
                    ModalityOverrideReason::ToolFailure,
                );
            }
        }
    }

    if !delivered_audio {
        event_log
            .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
                schema_version: 1,
                run_id: run_id.clone(),
                message: assistant_message.clone(),
                created_at_ms: now_ms(),
            }))
            .await?;
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            assistant_message.clone(),
            now_ms(),
        ))
        .await?;

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_assistant_message(&assistant_message).await {
            eprintln!("lcm ingest assistant failed for {}: {}", run_id.0, err);
        }
    }
    Ok(())
}

struct TypingGuard {
    telegram_tx: Sender<TelegramLoopEvent>,
    run_id: String,
}

impl TypingGuard {
    fn start(
        telegram_tx: Sender<TelegramLoopEvent>,
        run_id: String,
    ) -> Result<Self, mpsc::SendError<TelegramLoopEvent>> {
        telegram_tx.send(TelegramLoopEvent::TypingStart {
            run_id: run_id.clone(),
        })?;
        Ok(Self {
            telegram_tx,
            run_id,
        })
    }
}

impl Drop for TypingGuard {
    fn drop(&mut self) {
        let _ = self.telegram_tx.send(TelegramLoopEvent::TypingStop {
            run_id: self.run_id.clone(),
        });
    }
}

async fn run_agent_loop(
    runtime: Arc<RuntimeOrchestrator>,
    guidance_model: Arc<dyn GuidanceModel>,
    response_model: Arc<dyn ResponseModel>,
    summary_model: Arc<dyn SummaryModel>,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: Arc<FanoutRuntimeEventLog>,
    cfg: AppConfig,
    telegram_tx: Sender<TelegramLoopEvent>,
    mut prompt_rx: tokio_mpsc::UnboundedReceiver<IngressPrompt>,
) {
    let semaphore = Arc::new(Semaphore::new(cfg.max_concurrent_turns));
    let next_run_id = Arc::new(AtomicU64::new(1));

    eprintln!(
        "sieve-app agent mode ready; prompts accepted from stdin + Telegram chat {}",
        cfg.telegram_chat_id
    );

    while let Some(prompt) = prompt_rx.recv().await {
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };

        let runtime = runtime.clone();
        let guidance_model = guidance_model.clone();
        let response_model = response_model.clone();
        let summary_model = summary_model.clone();
        let lcm = lcm.clone();
        let event_log = event_log.clone();
        let cfg = cfg.clone();
        let telegram_tx = telegram_tx.clone();
        let source = prompt.source;
        let text = prompt.text;
        let modality = prompt.modality;
        let media_file_id = prompt.media_file_id;
        let run_index = next_run_id.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _permit = permit;
            let typing_guard = if source == PromptSource::Telegram {
                TypingGuard::start(telegram_tx, format!("run-{run_index}"))
                    .map(Some)
                    .unwrap_or(None)
            } else {
                None
            };
            if let Err(err) = run_turn(
                &runtime,
                guidance_model.as_ref(),
                response_model.as_ref(),
                summary_model.as_ref(),
                lcm.clone(),
                &event_log,
                &cfg,
                run_index,
                source,
                modality,
                media_file_id,
                text,
            )
            .await
            {
                eprintln!("run-{run_index} ({}) failed: {err}", source.as_str());
            }
            drop(typing_guard);
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv_if_present().map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let cli_prompt = env::args().skip(1).collect::<Vec<String>>().join(" ");
    let single_command_mode = !cli_prompt.trim().is_empty();

    let mut cfg =
        AppConfig::from_env().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let policy_toml = fs::read_to_string(&cfg.policy_path)?;
    let policy = TomlPolicyEngine::from_toml_str(&policy_toml)?;
    cfg.allowed_net_connect_scopes = planner_allowed_net_connect_scopes(&policy);
    let lcm = if cfg.lcm.enabled {
        Some(Arc::new(
            LcmIntegration::new(cfg.lcm.clone())
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?,
        ))
    } else {
        None
    };

    let planner = OpenAiPlannerModel::from_env()?;
    let guidance_model: Arc<dyn GuidanceModel> = Arc::new(OpenAiGuidanceModel::from_env()?);
    let response_model: Arc<dyn ResponseModel> = Arc::new(OpenAiResponseModel::from_env()?);
    let summary_model: Arc<dyn SummaryModel> = Arc::new(OpenAiSummaryModel::from_env()?);
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let (event_tx, event_rx) = mpsc::channel();
    let (prompt_rx, _stdin_thread, bridge) = if single_command_mode {
        (None, None, RuntimeBridge::new(approval_bus.clone()))
    } else {
        let (prompt_tx, prompt_rx) = tokio_mpsc::unbounded_channel();
        let stdin_thread = spawn_stdin_prompt_loop(prompt_tx.clone());
        (
            Some(prompt_rx),
            Some(stdin_thread),
            RuntimeBridge::with_prompt_tx(approval_bus.clone(), prompt_tx),
        )
    };
    let telegram_thread = spawn_telegram_loop(&cfg, bridge, event_rx);
    let typing_tx = event_tx.clone();
    let event_log = Arc::new(FanoutRuntimeEventLog::new(
        cfg.event_log_path.clone(),
        event_tx,
    )?);

    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
        quarantine: Arc::new(BwrapQuarantineRunner::default()),
        mainline: Arc::new(AppMainlineRunner::new(cfg.sieve_home.join("artifacts"))),
        planner: Arc::new(planner),
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    }));
    let allowances_path = approval_allowances_path(&cfg.sieve_home);
    match load_approval_allowances(&allowances_path) {
        Ok(allowances) => {
            if let Err(err) = runtime.restore_persistent_approval_allowances(&allowances) {
                eprintln!(
                    "failed to restore approval allowances from {}: {}",
                    allowances_path.display(),
                    err
                );
            }
        }
        Err(err) => {
            eprintln!(
                "failed to load approval allowances from {}: {}",
                allowances_path.display(),
                err
            );
        }
    }

    if single_command_mode {
        run_turn(
            &runtime,
            guidance_model.as_ref(),
            response_model.as_ref(),
            summary_model.as_ref(),
            lcm.clone(),
            &event_log,
            &cfg,
            1,
            PromptSource::Stdin,
            InteractionModality::Text,
            None,
            cli_prompt,
        )
        .await?;
        drop(runtime);
        drop(event_log);
        let _ = telegram_thread.join();
    } else {
        run_agent_loop(
            runtime.clone(),
            guidance_model.clone(),
            response_model.clone(),
            summary_model.clone(),
            lcm.clone(),
            event_log.clone(),
            cfg.clone(),
            typing_tx,
            prompt_rx.expect("agent mode prompt receiver missing"),
        )
        .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use sieve_interface_telegram::{
        TelegramAdapter as TestTelegramAdapter, TelegramAdapterConfig, TelegramLongPoll,
        TelegramMessage as TestTelegramMessage, TelegramUpdate as TestTelegramUpdate,
    };
    use sieve_llm::{GuidanceModel, LlmError, PlannerModel};
    use sieve_runtime::ApprovalBus;
    use sieve_types::{
        ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, CommandSegment, LlmModelConfig,
        LlmProvider, PlannerGuidanceFrame, PlannerGuidanceInput, PlannerGuidanceOutput,
        PlannerGuidanceSignal, PlannerToolCall, PlannerTurnInput, PlannerTurnOutput,
        PolicyDecision, PolicyDecisionKind, PolicyEvaluatedEvent, Resource,
    };
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::mpsc::TryRecvError;
    use std::sync::{Mutex as StdMutex, OnceLock};
    use tokio::time::{timeout, Duration};

    fn env_test_lock() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    struct EchoSummaryModel;

    #[async_trait]
    impl SummaryModel for EchoSummaryModel {
        fn config(&self) -> &sieve_types::LlmModelConfig {
            static CONFIG: OnceLock<sieve_types::LlmModelConfig> = OnceLock::new();
            CONFIG.get_or_init(|| sieve_types::LlmModelConfig {
                provider: sieve_types::LlmProvider::OpenAi,
                model: "summary-test".to_string(),
                api_base: None,
            })
        }

        async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError> {
            if request.ref_id.starts_with("assistant-compose-quality:") {
                return Ok("PASS".to_string());
            }
            if request.ref_id.starts_with("assistant-compose-grounding:") {
                return Ok("PASS".to_string());
            }
            if request.ref_id.starts_with("assistant-compose-gate:") {
                return Ok(
                    "{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":null}".to_string(),
                );
            }
            if request.ref_id.starts_with("assistant-compose:")
                || request.ref_id.starts_with("assistant-compose-retry:")
                || request
                    .ref_id
                    .starts_with("assistant-compose-grounded-retry:")
            {
                if let Ok(payload) = serde_json::from_str::<Value>(&request.content) {
                    if let Some(draft) = payload
                        .get("assistant_draft_message")
                        .and_then(Value::as_str)
                    {
                        return Ok(draft.to_string());
                    }
                    if let Some(previous) = payload
                        .get("previous_composed_message")
                        .and_then(Value::as_str)
                    {
                        return Ok(previous.to_string());
                    }
                }
                return Ok(String::new());
            }
            Ok(format!(
                "summary(bytes={},lines={})",
                request.byte_count, request.line_count
            ))
        }
    }

    struct PassThroughSummaryModel;

    #[async_trait]
    impl SummaryModel for PassThroughSummaryModel {
        fn config(&self) -> &sieve_types::LlmModelConfig {
            static CONFIG: OnceLock<sieve_types::LlmModelConfig> = OnceLock::new();
            CONFIG.get_or_init(|| sieve_types::LlmModelConfig {
                provider: sieve_types::LlmProvider::OpenAi,
                model: "summary-pass-through-test".to_string(),
                api_base: None,
            })
        }

        async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError> {
            if request.ref_id.starts_with("assistant-compose-quality:") {
                return Ok("PASS".to_string());
            }
            if request.ref_id.starts_with("assistant-compose-grounding:") {
                return Ok("PASS".to_string());
            }
            if request.ref_id.starts_with("assistant-compose-gate:") {
                return Ok(
                    "{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":null}".to_string(),
                );
            }
            if request.ref_id.starts_with("assistant-compose:")
                || request.ref_id.starts_with("assistant-compose-retry:")
                || request
                    .ref_id
                    .starts_with("assistant-compose-grounded-retry:")
            {
                if let Ok(payload) = serde_json::from_str::<Value>(&request.content) {
                    if let Some(draft) = payload
                        .get("assistant_draft_message")
                        .and_then(Value::as_str)
                    {
                        return Ok(draft.to_string());
                    }
                    if let Some(previous) = payload
                        .get("previous_composed_message")
                        .and_then(Value::as_str)
                    {
                        return Ok(previous.to_string());
                    }
                }
                return Ok(String::new());
            }
            Ok(request.content)
        }
    }

    struct QueuedSummaryModel {
        config: LlmModelConfig,
        outputs: StdMutex<VecDeque<Result<String, LlmError>>>,
        calls: AtomicU64,
    }

    impl QueuedSummaryModel {
        fn new(outputs: Vec<Result<String, LlmError>>) -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "summary-queue-test".to_string(),
                    api_base: None,
                },
                outputs: StdMutex::new(VecDeque::from(outputs)),
                calls: AtomicU64::new(0),
            }
        }

        fn call_count(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl SummaryModel for QueuedSummaryModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn summarize_ref(&self, _request: SummaryRequest) -> Result<String, LlmError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.outputs
                .lock()
                .expect("summary queue mutex poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(LlmError::Backend(
                        "summary queue exhausted with no configured output".to_string(),
                    ))
                })
        }
    }

    const E2E_POLICY_BASE: &str = r#"
[options]
violation_mode = "ask"
trusted_control = true
require_trusted_control_for_mutating = true

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://api.search.brave.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://markdown.new/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://weather.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.weather.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://forecast.weather.gov/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://wunderground.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.wunderground.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://google.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.google.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://x.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.x.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://api.open-meteo.com/"
"#;

    struct QueuedPlannerModel {
        config: LlmModelConfig,
        outputs: StdMutex<VecDeque<Result<PlannerTurnOutput, LlmError>>>,
        calls: AtomicU64,
    }

    impl QueuedPlannerModel {
        fn new(outputs: Vec<Result<PlannerTurnOutput, LlmError>>) -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "planner-test".to_string(),
                    api_base: None,
                },
                outputs: StdMutex::new(VecDeque::from(outputs)),
                calls: AtomicU64::new(0),
            }
        }

        fn call_count(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl PlannerModel for QueuedPlannerModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn plan_turn(&self, _input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.outputs
                .lock()
                .expect("planner queue mutex poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(LlmError::Backend(
                        "planner queue exhausted with no configured output".to_string(),
                    ))
                })
        }
    }

    struct QueuedGuidanceModel {
        config: LlmModelConfig,
        outputs: StdMutex<VecDeque<Result<PlannerGuidanceOutput, LlmError>>>,
    }

    impl QueuedGuidanceModel {
        fn new(outputs: Vec<Result<PlannerGuidanceOutput, LlmError>>) -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "guidance-test".to_string(),
                    api_base: None,
                },
                outputs: StdMutex::new(VecDeque::from(outputs)),
            }
        }
    }

    #[async_trait]
    impl GuidanceModel for QueuedGuidanceModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn classify_guidance(
            &self,
            _input: PlannerGuidanceInput,
        ) -> Result<PlannerGuidanceOutput, LlmError> {
            self.outputs
                .lock()
                .expect("guidance queue mutex poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Ok(PlannerGuidanceOutput {
                        guidance: PlannerGuidanceFrame {
                            code: PlannerGuidanceSignal::FinalAnswerReady.code(),
                            confidence_bps: 10_000,
                            source_hit_index: None,
                            evidence_ref_index: None,
                        },
                    })
                })
        }
    }

    fn guidance_output(signal: PlannerGuidanceSignal) -> PlannerGuidanceOutput {
        PlannerGuidanceOutput {
            guidance: PlannerGuidanceFrame {
                code: signal.code(),
                confidence_bps: 10_000,
                source_hit_index: None,
                evidence_ref_index: None,
            },
        }
    }

    struct QueuedResponseModel {
        config: LlmModelConfig,
        outputs: StdMutex<VecDeque<Result<sieve_llm::ResponseTurnOutput, LlmError>>>,
    }

    impl QueuedResponseModel {
        fn new(outputs: Vec<Result<sieve_llm::ResponseTurnOutput, LlmError>>) -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "response-test".to_string(),
                    api_base: None,
                },
                outputs: StdMutex::new(VecDeque::from(outputs)),
            }
        }
    }

    #[async_trait]
    impl ResponseModel for QueuedResponseModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn write_turn_response(
            &self,
            _input: ResponseTurnInput,
        ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
            self.outputs
                .lock()
                .expect("response queue mutex poisoned")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(LlmError::Backend(
                        "response queue exhausted with no configured output".to_string(),
                    ))
                })
        }
    }

    struct FirstStdoutSummaryResponseModel {
        config: LlmModelConfig,
    }

    impl FirstStdoutSummaryResponseModel {
        fn new() -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "response-first-stdout-test".to_string(),
                    api_base: None,
                },
            }
        }
    }

    #[async_trait]
    impl ResponseModel for FirstStdoutSummaryResponseModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn write_turn_response(
            &self,
            input: ResponseTurnInput,
        ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
            let stdout_ref = input
                .tool_outcomes
                .iter()
                .flat_map(|outcome| outcome.refs.iter())
                .find(|metadata| metadata.kind == "stdout" && metadata.byte_count > 0)
                .map(|metadata| metadata.ref_id.clone())
                .ok_or_else(|| {
                    LlmError::Backend("missing stdout ref for response rendering".to_string())
                })?;
            Ok(sieve_llm::ResponseTurnOutput {
                message: format!("[[summary:{stdout_ref}]]"),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::from([stdout_ref]),
            })
        }
    }

    struct MemoryRecallResponseModel {
        config: LlmModelConfig,
    }

    impl MemoryRecallResponseModel {
        fn new() -> Self {
            Self {
                config: LlmModelConfig {
                    provider: LlmProvider::OpenAi,
                    model: "response-memory-recall-test".to_string(),
                    api_base: None,
                },
            }
        }
    }

    #[async_trait]
    impl ResponseModel for MemoryRecallResponseModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn write_turn_response(
            &self,
            input: ResponseTurnInput,
        ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
            let lower_prompt = input.trusted_user_message.to_ascii_lowercase();
            let lower_thoughts = input
                .planner_thoughts
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase();
            let message = if lower_prompt.contains("where do i live") {
                if lower_thoughts.contains("hi i live in livermore ca") {
                    "You live in Livermore ca.".to_string()
                } else {
                    "I don't know where you live.".to_string()
                }
            } else {
                "Thanks for sharing.".to_string()
            };

            Ok(sieve_llm::ResponseTurnOutput {
                message,
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            })
        }
    }

    enum E2eModelMode {
        Fake {
            planner: Arc<dyn PlannerModel>,
            guidance: Arc<dyn GuidanceModel>,
            response: Arc<dyn ResponseModel>,
            summary: Arc<dyn SummaryModel>,
        },
        RealOpenAi,
    }

    #[derive(Clone, Default)]
    struct SharedTelegramPoller {
        updates: Arc<StdMutex<VecDeque<Vec<TestTelegramUpdate>>>>,
        sent_messages: Arc<StdMutex<Vec<(i64, String)>>>,
        sent_chat_actions: Arc<StdMutex<Vec<(i64, String)>>>,
        next_message_id: Arc<StdMutex<i64>>,
    }

    impl SharedTelegramPoller {
        fn push_updates(&self, updates: Vec<TestTelegramUpdate>) {
            self.updates
                .lock()
                .expect("telegram updates mutex poisoned")
                .push_back(updates);
        }

        fn sent_messages(&self) -> Vec<(i64, String)> {
            self.sent_messages
                .lock()
                .expect("telegram sent messages mutex poisoned")
                .clone()
        }

        fn sent_chat_actions(&self) -> Vec<(i64, String)> {
            self.sent_chat_actions
                .lock()
                .expect("telegram sent chat actions mutex poisoned")
                .clone()
        }
    }

    impl TelegramLongPoll for SharedTelegramPoller {
        fn get_updates(
            &mut self,
            _offset: Option<i64>,
            _timeout_secs: u16,
        ) -> Result<Vec<TestTelegramUpdate>, String> {
            Ok(self
                .updates
                .lock()
                .expect("telegram updates mutex poisoned")
                .pop_front()
                .unwrap_or_default())
        }

        fn send_message(&mut self, chat_id: i64, text: &str) -> Result<Option<i64>, String> {
            self.sent_messages
                .lock()
                .expect("telegram sent messages mutex poisoned")
                .push((chat_id, text.to_string()));
            let mut next_message_id = self
                .next_message_id
                .lock()
                .expect("telegram next message id mutex poisoned");
            let message_id = *next_message_id;
            *next_message_id += 1;
            Ok(Some(message_id))
        }

        fn send_chat_action(&mut self, chat_id: i64, action: &str) -> Result<(), String> {
            self.sent_chat_actions
                .lock()
                .expect("telegram sent chat actions mutex poisoned")
                .push((chat_id, action.to_string()));
            Ok(())
        }
    }

    struct TelegramFlowResult {
        sent_messages: Vec<(i64, String)>,
        sent_chat_actions: Vec<(i64, String)>,
    }

    struct AppE2eHarness {
        runtime: Arc<RuntimeOrchestrator>,
        approval_bus: Arc<InProcessApprovalBus>,
        guidance_model: Arc<dyn GuidanceModel>,
        response_model: Arc<dyn ResponseModel>,
        summary_model: Arc<dyn SummaryModel>,
        lcm: Option<Arc<LcmIntegration>>,
        event_log: Arc<FanoutRuntimeEventLog>,
        telegram_event_tx: Sender<TelegramLoopEvent>,
        telegram_event_rx: StdMutex<Receiver<TelegramLoopEvent>>,
        cfg: AppConfig,
        next_run_index: AtomicU64,
        root: PathBuf,
    }

    impl Drop for AppE2eHarness {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    impl AppE2eHarness {
        fn unique_root(prefix: &str) -> PathBuf {
            static NEXT_ID: AtomicU64 = AtomicU64::new(1);
            let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "{prefix}-{}-{}-{}",
                std::process::id(),
                now_ms(),
                unique
            ));
            fs::create_dir_all(&root).expect("create e2e harness root");
            root
        }

        fn new(model_mode: E2eModelMode, allowed_tools: Vec<String>, policy_toml: &str) -> Self {
            let root = Self::unique_root("sieve-app-e2e");
            let event_log_path = root.join("logs/runtime-events.jsonl");
            let mut cfg = AppConfig {
                telegram_bot_token: "test-token".to_string(),
                telegram_chat_id: 42,
                telegram_poll_timeout_secs: 1,
                telegram_allowed_sender_user_ids: None,
                sieve_home: root.clone(),
                policy_path: PathBuf::from(DEFAULT_POLICY_PATH),
                event_log_path: event_log_path.clone(),
                runtime_cwd: root.to_string_lossy().to_string(),
                allowed_tools,
                allowed_net_connect_scopes: Vec::new(),
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
                max_concurrent_turns: 1,
                max_planner_steps: 3,
                max_summary_calls_per_turn: 12,
                lcm: {
                    let mut lcm = LcmIntegrationConfig::from_sieve_home(&root);
                    lcm.enabled = false;
                    lcm
                },
            };

            let (guidance_model, response_model, summary_model, planner): (
                Arc<dyn GuidanceModel>,
                Arc<dyn ResponseModel>,
                Arc<dyn SummaryModel>,
                Arc<dyn PlannerModel>,
            ) = match model_mode {
                E2eModelMode::Fake {
                    planner,
                    guidance,
                    response,
                    summary,
                } => (guidance, response, summary, planner),
                E2eModelMode::RealOpenAi => (
                    Arc::new(OpenAiGuidanceModel::from_env().expect("load real guidance model")),
                    Arc::new(OpenAiResponseModel::from_env().expect("load real response model")),
                    Arc::new(OpenAiSummaryModel::from_env().expect("load real summary model")),
                    Arc::new(OpenAiPlannerModel::from_env().expect("load real planner model")),
                ),
            };

            let policy =
                TomlPolicyEngine::from_toml_str(policy_toml).expect("parse e2e harness policy");
            cfg.allowed_net_connect_scopes = planner_allowed_net_connect_scopes(&policy);
            let (telegram_event_tx, telegram_event_rx) = mpsc::channel();
            let event_log = Arc::new(
                FanoutRuntimeEventLog::new(event_log_path, telegram_event_tx.clone())
                    .expect("create e2e fanout event log"),
            );
            let approval_bus = Arc::new(InProcessApprovalBus::new());
            let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
                shell: Arc::new(BasicShellAnalyzer),
                summaries: Arc::new(DefaultCommandSummarizer),
                policy: Arc::new(policy),
                quarantine: Arc::new(BwrapQuarantineRunner::default()),
                mainline: Arc::new(AppMainlineRunner::new(cfg.sieve_home.join("artifacts"))),
                planner,
                approval_bus: approval_bus.clone(),
                event_log: event_log.clone(),
                clock: Arc::new(RuntimeClock),
            }));

            Self {
                runtime,
                approval_bus,
                guidance_model,
                response_model,
                summary_model,
                lcm: None,
                event_log,
                telegram_event_tx,
                telegram_event_rx: StdMutex::new(telegram_event_rx),
                cfg,
                next_run_index: AtomicU64::new(1),
                root,
            }
        }

        fn live_openai_or_skip(allowed_tools: Vec<String>) -> Option<Self> {
            if std::env::var("SIEVE_RUN_OPENAI_LIVE").ok().as_deref() != Some("1") {
                return None;
            }

            Some(Self::new(
                E2eModelMode::RealOpenAi,
                allowed_tools,
                E2E_POLICY_BASE,
            ))
        }

        fn with_lcm(mut self, lcm: Option<Arc<LcmIntegration>>) -> Self {
            self.lcm = lcm;
            self
        }

        async fn run_text_turn(&self, prompt: &str) -> Result<(), String> {
            let run_index = self.next_run_index.fetch_add(1, Ordering::Relaxed);
            run_turn(
                &self.runtime,
                self.guidance_model.as_ref(),
                self.response_model.as_ref(),
                self.summary_model.as_ref(),
                self.lcm.clone(),
                &self.event_log,
                &self.cfg,
                run_index,
                PromptSource::Stdin,
                InteractionModality::Text,
                None,
                prompt.to_string(),
            )
            .await
            .map_err(|err| err.to_string())
        }

        fn drain_telegram_events(
            &self,
            adapter: &mut TestTelegramAdapter<RuntimeBridge, SharedTelegramPoller, TelegramClock>,
        ) -> Result<(), String> {
            let receiver = self
                .telegram_event_rx
                .lock()
                .expect("telegram event receiver mutex poisoned");
            loop {
                match receiver.try_recv() {
                    Ok(TelegramLoopEvent::Runtime(event)) => {
                        adapter.publish_runtime_event(event).map_err(|err| {
                            format!("telegram publish runtime event failed: {err:?}")
                        })?
                    }
                    Ok(TelegramLoopEvent::TypingStart { run_id }) => {
                        adapter
                            .start_typing(run_id)
                            .map_err(|err| format!("telegram start typing failed: {err:?}"))?
                    }
                    Ok(TelegramLoopEvent::TypingStop { run_id }) => {
                        adapter.stop_typing(&run_id);
                    }
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
            Ok(())
        }

        async fn run_telegram_text_turn(&self, text: &str) -> Result<TelegramFlowResult, String> {
            let poller = SharedTelegramPoller::default();
            let (prompt_tx, mut prompt_rx) = tokio_mpsc::unbounded_channel();
            let bridge = RuntimeBridge::with_prompt_tx(self.approval_bus.clone(), prompt_tx);
            let mut adapter = TestTelegramAdapter::new(
                TelegramAdapterConfig {
                    chat_id: self.cfg.telegram_chat_id,
                    poll_timeout_secs: self.cfg.telegram_poll_timeout_secs,
                    allowed_sender_user_ids: self.cfg.telegram_allowed_sender_user_ids.clone(),
                },
                bridge,
                poller.clone(),
                TelegramClock,
            );

            poller.push_updates(vec![TestTelegramUpdate {
                update_id: 1,
                message: Some(TestTelegramMessage {
                    chat_id: self.cfg.telegram_chat_id,
                    sender_user_id: Some(1001),
                    message_id: 1001,
                    reply_to_message_id: None,
                    text: text.to_string(),
                }),
                message_reaction: None,
            }]);
            adapter
                .poll_once()
                .map_err(|err| format!("telegram poll failed: {err:?}"))?;

            let ingress = timeout(Duration::from_secs(1), prompt_rx.recv())
                .await
                .map_err(|_| "timed out waiting for telegram ingress prompt".to_string())?
                .ok_or_else(|| "telegram ingress prompt channel closed".to_string())?;

            let run_index = self.next_run_index.fetch_add(1, Ordering::Relaxed);
            let typing_guard =
                TypingGuard::start(self.telegram_event_tx.clone(), format!("run-{run_index}"))
                    .map(Some)
                    .unwrap_or(None);
            run_turn(
                &self.runtime,
                self.guidance_model.as_ref(),
                self.response_model.as_ref(),
                self.summary_model.as_ref(),
                self.lcm.clone(),
                &self.event_log,
                &self.cfg,
                run_index,
                PromptSource::Telegram,
                ingress.modality,
                ingress.media_file_id,
                ingress.text,
            )
            .await
            .map_err(|err| err.to_string())?;
            drop(typing_guard);

            self.drain_telegram_events(&mut adapter)?;
            Ok(TelegramFlowResult {
                sent_messages: poller.sent_messages(),
                sent_chat_actions: poller.sent_chat_actions(),
            })
        }

        fn runtime_events(&self) -> Vec<RuntimeEvent> {
            self.event_log.snapshot()
        }

        fn jsonl_records(&self) -> Vec<Value> {
            read_jsonl_records(&self.cfg.event_log_path)
        }
    }

    fn read_jsonl_records(path: &Path) -> Vec<Value> {
        let Ok(body) = fs::read_to_string(path) else {
            return Vec::new();
        };
        body.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }

    fn conversation_messages(records: &[Value]) -> Vec<(String, String)> {
        records
            .iter()
            .filter(|record| record.get("event").and_then(Value::as_str) == Some("conversation"))
            .map(|record| {
                (
                    record
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    record
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
    }

    fn assistant_messages(events: &[RuntimeEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                RuntimeEvent::AssistantMessage(event) => Some(event.message.clone()),
                _ => None,
            })
            .collect()
    }

    fn count_approval_requested(events: &[RuntimeEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ApprovalRequested(_)))
            .count()
    }

    fn assistant_errors_from_conversation(records: &[Value]) -> Vec<String> {
        conversation_messages(records)
            .into_iter()
            .filter(|(role, message)| role == "assistant" && message.starts_with("error:"))
            .map(|(_, message)| message)
            .collect()
    }

    fn message_contains_plain_url(message: &str) -> bool {
        message.contains("https://") || message.contains("http://")
    }

    fn latest_telegram_message(flow: &TelegramFlowResult) -> Option<&str> {
        flow.sent_messages
            .last()
            .map(|(_, message)| message.as_str())
    }

    fn message_has_weather_signal(message: &str) -> bool {
        let lower = message.to_ascii_lowercase();
        lower.contains("temp")
            || lower.contains("temperature")
            || lower.contains("°c")
            || lower.contains("°f")
            || lower.contains(" c ")
            || lower.contains(" f ")
            || lower.contains("rain")
            || lower.contains("precipitation")
            || lower.contains("high")
            || lower.contains("low")
            || lower.contains("cloud")
            || lower.contains("wind")
    }

    #[tokio::test]
    async fn e2e_fake_greeting_uses_guided_zero_tool_turn_without_approval() {
        let planner_output = PlannerTurnOutput {
            thoughts: Some("chat only".to_string()),
            tool_calls: Vec::new(),
        };
        let response_output = sieve_llm::ResponseTurnOutput {
            message: "Yes, I can hear you.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        };
        let planner: Arc<dyn PlannerModel> =
            Arc::new(QueuedPlannerModel::new(vec![Ok(planner_output)]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response: Arc<dyn ResponseModel> =
            Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Hi can you hear me?")
            .await
            .expect("greeting turn should succeed");

        let events = harness.runtime_events();
        assert_eq!(count_approval_requested(&events), 0);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
            "greeting should not trigger tool policy checks"
        );
        let assistant = assistant_messages(&events);
        assert_eq!(assistant, vec!["Yes, I can hear you.".to_string()]);

        let records = harness.jsonl_records();
        let conversation = conversation_messages(&records);
        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].0, "user");
        assert_eq!(conversation[1].0, "assistant");
        assert_eq!(conversation[1].1, "Yes, I can hear you.");
        assert!(
            assistant_errors_from_conversation(&records).is_empty(),
            "greeting flow should not emit assistant error conversation"
        );
    }

    #[tokio::test]
    async fn e2e_fake_lcm_does_not_auto_inject_trusted_memory_into_planner() {
        let _guard = env_test_lock()
            .lock()
            .expect("lcm recall env test lock poisoned");
        let previous_openai = std::env::var("OPENAI_API_KEY").ok();
        let previous_planner_openai = std::env::var("SIEVE_PLANNER_OPENAI_API_KEY").ok();
        std::env::set_var("OPENAI_API_KEY", "test-openai-key");
        std::env::remove_var("SIEVE_PLANNER_OPENAI_API_KEY");

        let planner: Arc<dyn PlannerModel> = Arc::new(QueuedPlannerModel::new(vec![
            Ok(PlannerTurnOutput {
                thoughts: None,
                tool_calls: Vec::new(),
            }),
            Ok(PlannerTurnOutput {
                thoughts: None,
                tool_calls: Vec::new(),
            }),
        ]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
            Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
            Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
        ]));
        let response: Arc<dyn ResponseModel> = Arc::new(MemoryRecallResponseModel::new());
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        let mut lcm_config = LcmIntegrationConfig::from_sieve_home(&harness.root);
        lcm_config.enabled = true;
        let lcm = Arc::new(LcmIntegration::new(lcm_config).expect("initialize lcm integration"));
        let harness = harness.with_lcm(Some(lcm));

        harness
            .run_text_turn("Hi I live in Livermore ca")
            .await
            .expect("first memory turn should succeed");
        harness
            .run_text_turn("Where do I live?")
            .await
            .expect("follow-up turn should succeed");

        let assistant = assistant_messages(&harness.runtime_events());
        assert!(
            assistant
                .iter()
                .any(|message| message.contains("I don't know where you live")),
            "without explicit memory tool use, planner should not receive injected trusted memory"
        );

        match previous_openai {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
        match previous_planner_openai {
            Some(value) => std::env::set_var("SIEVE_PLANNER_OPENAI_API_KEY", value),
            None => std::env::remove_var("SIEVE_PLANNER_OPENAI_API_KEY"),
        }
    }

    #[tokio::test]
    async fn telegram_full_flow_greeting_polls_ingress_and_sends_chat_reply() {
        let planner: Arc<dyn PlannerModel> =
            Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
                thoughts: Some("chat only".to_string()),
                tool_calls: Vec::new(),
            })]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
            sieve_llm::ResponseTurnOutput {
                message: "I'm doing well, thank you!".to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            },
        )]));
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        let flow = harness
            .run_telegram_text_turn("Hi how are you?")
            .await
            .expect("telegram full-flow greeting should succeed");

        assert!(
            flow.sent_messages
                .iter()
                .any(|(chat_id, message)| *chat_id == 42
                    && message.contains("I'm doing well, thank you!")),
            "assistant message should be sent via telegram sendMessage"
        );
        assert!(
            flow.sent_chat_actions
                .iter()
                .any(|(chat_id, action)| *chat_id == 42 && action == "typing"),
            "telegram typing action should be emitted during turn execution"
        );
        assert!(
            !harness
                .runtime_events()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
            "chat-only greeting should not dispatch tools"
        );
    }

    #[tokio::test]
    async fn telegram_full_flow_weather_runs_bash_and_sends_weather_text() {
        let planner: Arc<dyn PlannerModel> = Arc::new(QueuedPlannerModel::new(vec![Ok(
            PlannerTurnOutput {
                thoughts: Some("fetch weather".to_string()),
                tool_calls: vec![PlannerToolCall {
                    tool_name: "bash".to_string(),
                    args: BTreeMap::from([(
                        "cmd".to_string(),
                        serde_json::json!(
                            "echo 'Dublin weather today: 12C and cloudy'; echo 'https://weather.example.test/dublin-today'"
                        ),
                    )]),
                }],
            },
        )]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response: Arc<dyn ResponseModel> = Arc::new(FirstStdoutSummaryResponseModel::new());
        let summary: Arc<dyn SummaryModel> = Arc::new(PassThroughSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec!["bash".to_string()],
            E2E_POLICY_BASE,
        );

        let flow = harness
            .run_telegram_text_turn("weather in dublin ireland today")
            .await
            .expect("telegram full-flow weather should succeed");

        assert!(
            flow.sent_messages.iter().any(|(_, message)| {
                let lower = message.to_ascii_lowercase();
                lower.contains("dublin weather today")
                    && lower.contains("12c")
                    && message.contains("https://weather.example.test/dublin-today")
            }),
            "assistant telegram reply should include rendered weather result and source URL"
        );
        assert!(
            flow.sent_chat_actions
                .iter()
                .any(|(chat_id, action)| *chat_id == 42 && action == "typing"),
            "telegram typing action should be emitted during weather turn"
        );
        assert!(
            harness
                .runtime_events()
                .iter()
                .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
            "weather request should exercise runtime tool/policy path"
        );
    }

    #[tokio::test]
    async fn e2e_fake_greeting_runs_general_planner_loop_without_tools() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("friendly response".to_string()),
            tool_calls: Vec::new(),
        })]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response_output = sieve_llm::ResponseTurnOutput {
            message: "I'm doing well, thank you!".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        };
        let response: Arc<dyn ResponseModel> =
            Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner: planner.clone(),
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Hi how are you?")
            .await
            .expect("guided greeting should succeed");

        assert_eq!(
            planner.call_count(),
            1,
            "greeting should still run planner loop once in general mode"
        );
        let events = harness.runtime_events();
        assert_eq!(count_approval_requested(&events), 0);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
            "zero-tool greeting should avoid tool policy checks"
        );
    }

    #[tokio::test]
    async fn e2e_fake_guidance_continue_executes_multiple_planner_steps() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![
            Ok(PlannerTurnOutput {
                thoughts: Some("step-1".to_string()),
                tool_calls: Vec::new(),
            }),
            Ok(PlannerTurnOutput {
                thoughts: Some("step-2".to_string()),
                tool_calls: Vec::new(),
            }),
        ]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
            Ok(guidance_output(PlannerGuidanceSignal::ContinueNeedEvidence)),
            Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
        ]));
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
            sieve_llm::ResponseTurnOutput {
                message: "Two-step complete.".to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            },
        )]));
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner: planner.clone(),
                guidance,
                response,
                summary,
            },
            vec!["bash".to_string()],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Gather more context and then answer.")
            .await
            .expect("multi-step guided turn should succeed");

        assert_eq!(
            planner.call_count(),
            2,
            "guidance continue should run step 2"
        );
    }

    #[tokio::test]
    async fn e2e_fake_guidance_continue_stops_after_two_empty_steps() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![
            Ok(PlannerTurnOutput {
                thoughts: Some("step-1".to_string()),
                tool_calls: Vec::new(),
            }),
            Ok(PlannerTurnOutput {
                thoughts: Some("step-2".to_string()),
                tool_calls: Vec::new(),
            }),
            Ok(PlannerTurnOutput {
                thoughts: Some("step-3".to_string()),
                tool_calls: Vec::new(),
            }),
        ]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
            Ok(guidance_output(PlannerGuidanceSignal::ContinueNeedEvidence)),
            Ok(guidance_output(
                PlannerGuidanceSignal::ContinueFetchAdditionalSource,
            )),
            Ok(guidance_output(
                PlannerGuidanceSignal::ContinueFetchAdditionalSource,
            )),
        ]));
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
            sieve_llm::ResponseTurnOutput {
                message: "Stopped early.".to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            },
        )]));
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner: planner.clone(),
                guidance,
                response,
                summary,
            },
            vec!["bash".to_string()],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Keep searching until done.")
            .await
            .expect("empty-step guard turn should succeed");

        assert_eq!(
            planner.call_count(),
            2,
            "two consecutive empty planner steps should stop loop"
        );
    }

    #[tokio::test]
    async fn e2e_fake_compose_continue_stops_when_no_new_evidence() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![
            Ok(PlannerTurnOutput {
                thoughts: Some("step-1".to_string()),
                tool_calls: Vec::new(),
            }),
            Ok(PlannerTurnOutput {
                thoughts: Some("step-2".to_string()),
                tool_calls: Vec::new(),
            }),
        ]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
            Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
            Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
        ]));
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![
            Ok(sieve_llm::ResponseTurnOutput {
                message: "Draft one".to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            }),
            Ok(sieve_llm::ResponseTurnOutput {
                message: "Draft two".to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            }),
        ]));
        let summary_impl = Arc::new(QueuedSummaryModel::new(vec![
            Ok("Cycle 1 response.".to_string()),
            Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":102}".to_string()),
            Ok("Cycle 2 response.".to_string()),
            Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":102}".to_string()),
        ]));
        let summary: Arc<dyn SummaryModel> = summary_impl.clone();
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner: planner.clone(),
                guidance,
                response,
                summary,
            },
            vec!["bash".to_string()],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Hi I live in Livermore ca")
            .await
            .expect("compose no-new-evidence guard turn should succeed");

        assert_eq!(
            planner.call_count(),
            2,
            "compose follow-up should run once, then stop on repeated evidence"
        );
        assert_eq!(
            summary_impl.call_count(),
            4,
            "compose pass should be bounded to two cycles (compose+gate each)"
        );
        let assistant = assistant_messages(&harness.runtime_events());
        assert_eq!(
            assistant.last().map(String::as_str),
            Some("Cycle 2 response."),
            "second compose cycle response should be final output"
        );
    }

    #[tokio::test]
    async fn e2e_fake_compose_summary_budget_caps_calls() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("step-1".to_string()),
            tool_calls: Vec::new(),
        })]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
            sieve_llm::ResponseTurnOutput {
                message: "Draft one".to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            },
        )]));
        let summary_impl = Arc::new(QueuedSummaryModel::new(vec![
            Ok("Budgeted response.".to_string()),
            Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":102}".to_string()),
        ]));
        let summary: Arc<dyn SummaryModel> = summary_impl.clone();
        let mut harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner: planner.clone(),
                guidance,
                response,
                summary,
            },
            vec!["bash".to_string()],
            E2E_POLICY_BASE,
        );
        harness.cfg.max_summary_calls_per_turn = 2;

        harness
            .run_text_turn("Hi I live in Livermore ca")
            .await
            .expect("compose summary budget turn should succeed");

        assert_eq!(
            planner.call_count(),
            1,
            "summary budget should block additional compose follow-up cycles"
        );
        assert_eq!(
            summary_impl.call_count(),
            2,
            "summary calls should stop at configured per-turn budget"
        );
        let assistant = assistant_messages(&harness.runtime_events());
        assert_eq!(
            assistant.last().map(String::as_str),
            Some("Budgeted response."),
            "budgeted compose response should still render a final assistant reply"
        );
    }

    #[tokio::test]
    async fn e2e_fake_general_compose_pass_rewrites_final_message() {
        let planner_output = PlannerTurnOutput {
            thoughts: Some("direct response".to_string()),
            tool_calls: Vec::new(),
        };
        let response_output = sieve_llm::ResponseTurnOutput {
            message: "Draft response that is too wordy.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        };
        let planner: Arc<dyn PlannerModel> =
            Arc::new(QueuedPlannerModel::new(vec![Ok(planner_output)]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response: Arc<dyn ResponseModel> =
            Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
        let summary: Arc<dyn SummaryModel> = Arc::new(QueuedSummaryModel::new(vec![
            Ok("Hello there.".to_string()),
            Ok("PASS".to_string()),
        ]));
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Please greet me briefly.")
            .await
            .expect("compose pass turn should succeed");

        let assistant = assistant_messages(&harness.runtime_events());
        assert_eq!(assistant, vec!["Hello there.".to_string()]);
    }

    #[tokio::test]
    async fn e2e_fake_compose_retries_on_meta_narration() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("chat".to_string()),
            tool_calls: Vec::new(),
        })]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response_output = sieve_llm::ResponseTurnOutput {
            message: "Hey!".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        };
        let response: Arc<dyn ResponseModel> =
            Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
        let summary: Arc<dyn SummaryModel> = Arc::new(QueuedSummaryModel::new(vec![
            Ok("The assistant is ready to help and asks how it can assist.".to_string()),
            Ok("PASS".to_string()),
            Ok("I'm doing well, thanks for asking. How can I help?".to_string()),
        ]));
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Reply directly to this greeting: Hi how are you?")
            .await
            .expect("meta compose retry turn should succeed");

        let assistant = assistant_messages(&harness.runtime_events());
        assert_eq!(assistant.len(), 1);
        assert!(
            assistant[0].starts_with("I'm doing well"),
            "compose retry should replace third-person meta narration"
        );
    }

    #[tokio::test]
    async fn e2e_fake_compose_falls_back_to_draft_on_evidence_summary_diagnostic_leak() {
        let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("chat".to_string()),
            tool_calls: Vec::new(),
        })]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let draft =
            "Thanks for sharing that you live in Livermore, CA. What can I help with today?"
                .to_string();
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
            sieve_llm::ResponseTurnOutput {
                message: draft.clone(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            },
        )]));
        let summary: Arc<dyn SummaryModel> = Arc::new(QueuedSummaryModel::new(vec![
            Ok("The evidence summary explicitly says no relevant evidence was found, so stating the user’s location as a verified fact would be ungrounded.".to_string()),
            Ok("PASS".to_string()),
            Ok("The evidence summary explicitly says no relevant evidence was found.".to_string()),
            Ok("PASS".to_string()),
        ]));
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec![
                "bash".to_string(),
                "endorse".to_string(),
                "declassify".to_string(),
            ],
            E2E_POLICY_BASE,
        );

        harness
            .run_text_turn("Hi I live in Livermore ca")
            .await
            .expect("diagnostic leak turn should succeed");

        let assistant = assistant_messages(&harness.runtime_events());
        assert_eq!(assistant, vec![draft]);
    }

    #[tokio::test]
    async fn e2e_fake_planner_error_emits_assistant_error_for_user_visibility() {
        let planner: Arc<dyn PlannerModel> = Arc::new(QueuedPlannerModel::new(vec![Err(
            LlmError::Backend("planner boom".to_string()),
        )]));
        let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
            guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
        )]));
        let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(Vec::new()));
        let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
        let harness = AppE2eHarness::new(
            E2eModelMode::Fake {
                planner,
                guidance,
                response,
                summary,
            },
            vec!["bash".to_string()],
            E2E_POLICY_BASE,
        );

        let err = harness
            .run_text_turn("Use bash to run exactly: pwd")
            .await
            .expect_err("planner failure must propagate to caller");
        assert!(err.contains("planner model failed"));

        let events = harness.runtime_events();
        let assistant = assistant_messages(&events);
        assert_eq!(assistant.len(), 1);
        assert!(
            assistant[0].starts_with("error:"),
            "assistant-visible fallback must be emitted on planner failure"
        );

        let records = harness.jsonl_records();
        let assistant_errors = assistant_errors_from_conversation(&records);
        assert_eq!(assistant_errors.len(), 1);
        assert!(assistant_errors[0].contains("planner model failed"));
    }

    #[tokio::test]
    async fn live_e2e_greeting_stays_chat_only_env_gated() {
        let _guard = env_test_lock()
            .lock()
            .expect("live e2e env test lock poisoned");
        let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ]) else {
            return;
        };

        harness
            .run_text_turn("Hi can you hear me?")
            .await
            .expect("live greeting should succeed");

        let events = harness.runtime_events();
        assert_eq!(count_approval_requested(&events), 0);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
            "greeting should remain chat-only with zero tool dispatches"
        );
        let assistant = assistant_messages(&events);
        assert_eq!(assistant.len(), 1);
        assert!(
            !assistant[0].trim().is_empty() && !assistant[0].starts_with("error:"),
            "live greeting must produce a non-error assistant reply"
        );
        let records = harness.jsonl_records();
        assert!(
            assistant_errors_from_conversation(&records).is_empty(),
            "live greeting must not produce assistant error conversation entries"
        );
    }

    #[tokio::test]
    async fn live_telegram_full_flow_greeting_env_gated() {
        let _guard = env_test_lock()
            .lock()
            .expect("live e2e env test lock poisoned");
        let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ]) else {
            return;
        };

        let flow = harness
            .run_telegram_text_turn("Hi how are you?")
            .await
            .expect("live telegram greeting should succeed");
        let message =
            latest_telegram_message(&flow).expect("live telegram greeting should send message");

        assert!(
            !message.trim().is_empty() && !message.starts_with("error:"),
            "live telegram greeting must produce a non-error assistant reply"
        );
        assert!(
            !obvious_meta_compose_pattern(message),
            "live telegram greeting reply should be direct, not third-person meta"
        );
        assert!(
            flow.sent_chat_actions
                .iter()
                .any(|(_, action)| action == "typing"),
            "live telegram greeting should emit typing action"
        );
    }

    #[tokio::test]
    async fn live_telegram_full_flow_weather_today_env_gated() {
        let _guard = env_test_lock()
            .lock()
            .expect("live e2e env test lock poisoned");
        let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ]) else {
            return;
        };

        let flow = harness
            .run_telegram_text_turn("weather in dublin ireland today")
            .await
            .expect("live telegram weather today should succeed");
        let message = latest_telegram_message(&flow)
            .expect("live telegram weather today should send message");
        let lower = message.to_ascii_lowercase();

        assert!(
            !message.starts_with("error:") && !obvious_meta_compose_pattern(message),
            "live telegram weather today should produce direct non-error response"
        );
        assert!(
            message_contains_plain_url(message),
            "live telegram weather today response should include at least one plain URL"
        );
        assert!(
            message_has_weather_signal(message),
            "live telegram weather today response should include concrete weather signal"
        );
        assert!(
            lower.contains("today") || lower.contains("current") || lower.contains("now"),
            "live telegram weather today response should answer the requested timeframe"
        );
    }

    #[tokio::test]
    async fn live_telegram_full_flow_weather_tomorrow_env_gated() {
        let _guard = env_test_lock()
            .lock()
            .expect("live e2e env test lock poisoned");
        let Some(harness) = AppE2eHarness::live_openai_or_skip(vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ]) else {
            return;
        };

        let flow = harness
            .run_telegram_text_turn("weather in dublin ireland tomorrow")
            .await
            .expect("live telegram weather tomorrow should succeed");
        let message = latest_telegram_message(&flow)
            .expect("live telegram weather tomorrow should send message");
        let lower = message.to_ascii_lowercase();

        assert!(
            !message.starts_with("error:") && !obvious_meta_compose_pattern(message),
            "live telegram weather tomorrow should produce direct non-error response"
        );
        assert!(
            message_contains_plain_url(message),
            "live telegram weather tomorrow response should include at least one plain URL"
        );
        assert!(
            message_has_weather_signal(message),
            "live telegram weather tomorrow response should include concrete weather signal"
        );
        assert!(
            lower.contains("tomorrow"),
            "live telegram weather tomorrow response should answer the requested timeframe"
        );
    }

    #[tokio::test]
    async fn runtime_bridge_submit_approval_resolves_pending_request() {
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let bridge = RuntimeBridge::new(approval_bus.clone());
        let request_id = ApprovalRequestId("approval-test".to_string());
        approval_bus
            .publish_requested(ApprovalRequestedEvent {
                schema_version: 1,
                request_id: request_id.clone(),
                run_id: RunId("run-test".to_string()),
                command_segments: vec![CommandSegment {
                    argv: vec!["rm".to_string(), "-rf".to_string(), "/tmp/x".to_string()],
                    operator_before: None,
                }],
                inferred_capabilities: vec![sieve_types::Capability {
                    resource: Resource::Fs,
                    action: sieve_types::Action::Write,
                    scope: "/tmp/x".to_string(),
                }],
                blocked_rule_id: "rule".to_string(),
                reason: "reason".to_string(),
                created_at_ms: 1,
            })
            .await
            .expect("publish approval request");

        bridge.submit_approval(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: request_id.clone(),
            run_id: RunId("run-test".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2,
        });

        let resolved = approval_bus
            .wait_resolved(&request_id)
            .await
            .expect("wait resolved");
        assert_eq!(resolved.action, ApprovalAction::ApproveOnce);
    }

    #[tokio::test]
    async fn runtime_bridge_submit_prompt_enqueues_telegram_input() {
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let bridge = RuntimeBridge::with_prompt_tx(approval_bus, tx);

        bridge.submit_prompt(TelegramPrompt {
            chat_id: 42,
            text: "  check logs  ".to_string(),
            modality: InteractionModality::Text,
            media_file_id: None,
        });

        let prompt = rx.recv().await.expect("expected prompt");
        assert_eq!(prompt.source, PromptSource::Telegram);
        assert_eq!(prompt.text, "check logs");
        assert_eq!(prompt.modality, InteractionModality::Text);
        assert!(prompt.media_file_id.is_none());
    }

    #[tokio::test]
    async fn fanout_runtime_event_log_records_and_forwards_events() {
        let (tx, rx) = mpsc::channel();
        let path =
            std::env::temp_dir().join(format!("sieve-app-event-log-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&path);
        let log = FanoutRuntimeEventLog::new(path.clone(), tx).expect("create fanout log");
        let event = RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: RunId("run-log".to_string()),
            decision: PolicyDecision {
                kind: PolicyDecisionKind::Allow,
                reason: "allow".to_string(),
                blocked_rule_id: None,
            },
            inferred_capabilities: Vec::new(),
            trace_path: None,
            created_at_ms: 3,
        });

        log.append(event.clone()).await.expect("append event");
        assert_eq!(log.snapshot(), vec![event.clone()]);
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(50))
                .expect("forwarded event"),
            TelegramLoopEvent::Runtime(event.clone())
        );
        let body = fs::read_to_string(&path).expect("read jsonl log");
        assert!(body.contains("policy_evaluated"));
        log.append_conversation(ConversationLogRecord::new(
            RunId("run-log".to_string()),
            ConversationRole::User,
            "hello".to_string(),
            4,
        ))
        .await
        .expect("append conversation");
        let body = fs::read_to_string(&path).expect("read jsonl log");
        assert!(body.contains("\"event\":\"conversation\""));
        assert!(body.contains("\"message\":\"hello\""));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn typing_guard_emits_start_and_stop_events() {
        let (tx, rx) = mpsc::channel();
        {
            let _guard =
                TypingGuard::start(tx.clone(), "run-typing".to_string()).expect("start typing");
            assert_eq!(
                rx.recv_timeout(Duration::from_millis(50))
                    .expect("typing start event"),
                TelegramLoopEvent::TypingStart {
                    run_id: "run-typing".to_string()
                }
            );
        }

        assert_eq!(
            rx.recv_timeout(Duration::from_millis(50))
                .expect("typing stop event"),
            TelegramLoopEvent::TypingStop {
                run_id: "run-typing".to_string()
            }
        );
    }

    #[test]
    fn st_audio_stt_args_include_input_path() {
        let args = st_audio_stt_args(Path::new("/tmp/input.ogg"));
        let rendered = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<String>>();
        assert_eq!(
            rendered,
            vec!["stt".to_string(), "/tmp/input.ogg".to_string()]
        );
    }

    #[test]
    fn st_audio_tts_args_force_ogg_output_file() {
        let args = st_audio_tts_args(Path::new("/tmp/tts-input.txt"), Path::new("/tmp/out.ogg"));
        let rendered = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<String>>();
        assert_eq!(
            rendered,
            vec![
                "tts".to_string(),
                "/tmp/tts-input.txt".to_string(),
                "--format".to_string(),
                "ogg".to_string(),
                "--output".to_string(),
                "/tmp/out.ogg".to_string(),
            ]
        );
    }

    #[test]
    fn codex_image_ocr_args_include_read_only_ephemeral_image_prompt() {
        let args = codex_image_ocr_args(Path::new("/tmp/photo.png"));
        let rendered = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<String>>();
        assert_eq!(
            rendered,
            vec![
                "exec".to_string(),
                "--sandbox".to_string(),
                "read-only".to_string(),
                "--ephemeral".to_string(),
                "--image".to_string(),
                "/tmp/photo.png".to_string(),
                "--".to_string(),
                CODEX_IMAGE_OCR_PROMPT.to_string(),
            ]
        );
    }

    #[test]
    fn modality_contract_defaults_and_overrides() {
        let mut contract = default_modality_contract(InteractionModality::Audio);
        assert_eq!(contract.input, InteractionModality::Audio);
        assert_eq!(contract.response, InteractionModality::Audio);
        assert!(contract.override_reason.is_none());

        override_modality_contract(
            &mut contract,
            InteractionModality::Text,
            ModalityOverrideReason::ToolFailure,
        );
        assert_eq!(contract.response, InteractionModality::Text);
        assert_eq!(
            contract.override_reason,
            Some(ModalityOverrideReason::ToolFailure)
        );
    }

    #[test]
    fn parse_policy_path_uses_baseline_default_for_missing_or_blank() {
        assert_eq!(
            parse_policy_path(None),
            PathBuf::from("docs/policy/baseline-policy.toml")
        );
        assert_eq!(
            parse_policy_path(Some(String::new())),
            PathBuf::from("docs/policy/baseline-policy.toml")
        );
        assert_eq!(
            parse_policy_path(Some("   ".to_string())),
            PathBuf::from("docs/policy/baseline-policy.toml")
        );
    }

    #[test]
    fn parse_policy_path_honors_explicit_env_override() {
        assert_eq!(
            parse_policy_path(Some("custom/policy.toml".to_string())),
            PathBuf::from("custom/policy.toml")
        );
    }

    #[test]
    fn planner_allowed_tools_for_turn_hides_explicit_ref_tools_without_value_refs() {
        let configured = vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ];
        assert_eq!(
            planner_allowed_tools_for_turn(&configured, false),
            vec!["bash".to_string()]
        );
        assert_eq!(
            planner_allowed_tools_for_turn(&configured, true),
            configured
        );
    }

    #[test]
    fn planner_allowed_net_connect_scopes_prefers_origin_level_entries() {
        let policy = TomlPolicyEngine::from_toml_str(
            r#"
[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://forecast.weather.gov/MapClick.php?lat=37.7&lon=-122.4"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://forecast.weather.gov/hourly"

[[allow_capabilities]]
resource = "fs"
action = "read"
scope = "/tmp/input.txt"
"#,
        )
        .expect("parse policy");

        let scopes = planner_allowed_net_connect_scopes(&policy);
        assert_eq!(scopes, vec!["https://forecast.weather.gov".to_string()]);
    }

    #[test]
    fn parse_event_log_path_defaults_to_home_sieve_logs() {
        assert_eq!(
            runtime_event_log_path(&parse_sieve_home(None, Some("/home/alice".to_string()))),
            PathBuf::from("/home/alice/.sieve/logs/runtime-events.jsonl")
        );
    }

    #[test]
    fn parse_event_log_path_uses_sieve_home_when_set() {
        assert_eq!(
            runtime_event_log_path(&parse_sieve_home(
                Some("/var/sieve".to_string()),
                Some("/home/alice".to_string())
            )),
            PathBuf::from("/var/sieve/logs/runtime-events.jsonl")
        );
    }

    #[test]
    fn load_approval_allowances_missing_file_returns_empty() {
        let path = std::env::temp_dir().join(format!(
            "sieve-app-allowances-missing-{}-{}.json",
            std::process::id(),
            now_ms()
        ));
        let loaded = load_approval_allowances(&path).expect("missing file should be empty");
        assert!(loaded.is_empty());
    }

    #[test]
    fn approval_allowances_file_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "sieve-app-allowances-roundtrip-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let path = approval_allowances_path(&root);
        let allowances = vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://example.com".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/input.txt".to_string(),
            },
        ];
        save_approval_allowances(&path, &allowances).expect("save allowances");
        let loaded = load_approval_allowances(&path).expect("load allowances");
        assert_eq!(loaded, allowances);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn approval_allowances_parallel_saves_do_not_fail() {
        let root = std::env::temp_dir().join(format!(
            "sieve-app-allowances-parallel-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let path = approval_allowances_path(&root);
        let workers = 16usize;
        let rounds_per_worker = 12usize;
        let start = Arc::new(std::sync::Barrier::new(workers));
        let errors = Arc::new(StdMutex::new(Vec::new()));

        std::thread::scope(|scope| {
            for worker_idx in 0..workers {
                let path = path.clone();
                let start = Arc::clone(&start);
                let errors = Arc::clone(&errors);
                scope.spawn(move || {
                    start.wait();
                    for round in 0..rounds_per_worker {
                        let allowances = vec![Capability {
                            resource: Resource::Fs,
                            action: Action::Read,
                            scope: format!("/tmp/input-{worker_idx}-{round}.txt"),
                        }];
                        if let Err(err) = save_approval_allowances(&path, &allowances) {
                            errors.lock().expect("errors lock").push(err);
                        }
                    }
                });
            }
        });

        let failures = errors.lock().expect("errors lock").clone();
        assert!(
            failures.is_empty(),
            "parallel save failures: {}",
            failures.join("; ")
        );
        let loaded = load_approval_allowances(&path).expect("load final allowances");
        assert!(!loaded.is_empty(), "final allowances must exist");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_telegram_allowed_sender_user_ids_supports_missing_and_blank() {
        assert_eq!(parse_telegram_allowed_sender_user_ids(None), Ok(None));
        assert_eq!(
            parse_telegram_allowed_sender_user_ids(Some("   ".to_string())),
            Ok(None)
        );
    }

    #[test]
    fn parse_telegram_allowed_sender_user_ids_parses_csv() {
        let parsed = parse_telegram_allowed_sender_user_ids(Some("1001,-42,1001".to_string()))
            .expect("parse ids");
        assert_eq!(parsed, Some(BTreeSet::from([1001, -42])));
    }

    #[test]
    fn parse_telegram_allowed_sender_user_ids_rejects_invalid_entry() {
        let err = parse_telegram_allowed_sender_user_ids(Some("1001,nope".to_string()))
            .expect_err("must reject invalid user id");
        assert!(err.contains("invalid SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS entry `nope`"));
    }

    #[tokio::test]
    async fn render_assistant_message_replaces_known_tokens() {
        let message = "trace path: [[ref:trace:run-1]]";
        let refs = BTreeMap::from([(
            "trace:run-1".to_string(),
            RenderRef::Literal {
                value: "/tmp/sieve/trace/run-1".to_string(),
            },
        )]);
        let referenced_ref_ids = BTreeSet::from(["trace:run-1".to_string()]);
        let summarized_ref_ids = BTreeSet::new();

        let expanded = render_assistant_message(
            message,
            &referenced_ref_ids,
            &summarized_ref_ids,
            &refs,
            &EchoSummaryModel,
            &RunId("run-test".to_string()),
        )
        .await;
        assert_eq!(expanded, "trace path: /tmp/sieve/trace/run-1");
    }

    #[test]
    fn build_response_turn_input_handles_zero_tool_turn() {
        let run_id = RunId("run-1".to_string());
        let planner_result = PlannerRunResult {
            thoughts: Some("chat reply".to_string()),
            tool_results: Vec::new(),
        };

        let (input, refs) = build_response_turn_input(&run_id, "hi", &planner_result);
        assert_eq!(input.run_id, run_id);
        assert_eq!(input.trusted_user_message, "hi");
        assert_eq!(input.planner_thoughts.as_deref(), Some("chat reply"));
        assert!(input.tool_outcomes.is_empty());
        assert!(refs.is_empty());
    }

    #[test]
    fn requires_output_visibility_detects_non_empty_stdout_or_stderr_refs() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "show output".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some("pwd".to_string()),
                failure_reason: None,
                refs: vec![
                    ResponseRefMetadata {
                        ref_id: "artifact-1".to_string(),
                        kind: "stdout".to_string(),
                        byte_count: 42,
                        line_count: 2,
                    },
                    ResponseRefMetadata {
                        ref_id: "artifact-2".to_string(),
                        kind: "stderr".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                ],
            }],
        };

        assert!(requires_output_visibility(&input));
    }

    #[test]
    fn requires_output_visibility_skips_when_user_did_not_ask_for_output() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "What is the weather tomorrow in Livermore?".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some(
                    "bravesearch search --query \"Livermore CA weather tomorrow\" --count 5 --output json"
                        .to_string(),
                ),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 1024,
                    line_count: 12,
                }],
            }],
        };

        assert!(!requires_output_visibility(&input));
    }

    #[test]
    fn response_has_visible_selected_output_requires_message_token() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "show output".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some("pwd".to_string()),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 4,
                    line_count: 1,
                }],
            }],
        };

        let no_token = sieve_llm::ResponseTurnOutput {
            message: "completed".to_string(),
            referenced_ref_ids: BTreeSet::from(["artifact-1".to_string()]),
            summarized_ref_ids: BTreeSet::new(),
        };
        assert!(!response_has_visible_selected_output(&input, &no_token));

        let with_token = sieve_llm::ResponseTurnOutput {
            message: "output: [[ref:artifact-1]]".to_string(),
            referenced_ref_ids: BTreeSet::from(["artifact-1".to_string()]),
            summarized_ref_ids: BTreeSet::new(),
        };
        assert!(response_has_visible_selected_output(&input, &with_token));
    }

    #[test]
    fn response_has_visible_selected_output_accepts_summary_token() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "summarize output".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some("pwd".to_string()),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-2".to_string(),
                    kind: "stderr".to_string(),
                    byte_count: 10,
                    line_count: 2,
                }],
            }],
        };

        let response = sieve_llm::ResponseTurnOutput {
            message: "summary: [[summary:artifact-2]]".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::from(["artifact-2".to_string()]),
        };
        assert!(response_has_visible_selected_output(&input, &response));
    }

    #[test]
    fn compose_quality_retry_treats_verbose_pass_as_pass() {
        let composed = "Here is a direct answer.";
        let gate = Some("Quality gate verdict: PASS because the answer is direct.");
        assert!(compose_quality_requires_retry(composed, gate).is_none());
    }

    #[test]
    fn gate_requires_retry_treats_pass_as_no_retry() {
        assert!(gate_requires_retry(Some("PASS")).is_none());
        assert!(gate_requires_retry(Some("verdict: pass")).is_none());
        assert!(gate_requires_retry(Some("REVISE: unsupported claim")).is_some());
        assert!(
            gate_requires_retry(Some("This response lacks specific weather details.")).is_some()
        );
    }

    #[test]
    fn combine_gate_reasons_joins_non_empty_reasons() {
        let combined = combine_gate_reasons(&[
            Some("REVISE: quality".to_string()),
            None,
            Some("REVISE: grounding".to_string()),
        ]);
        assert_eq!(
            combined.as_deref(),
            Some("REVISE: quality | REVISE: grounding")
        );
    }

    #[test]
    fn denied_outcomes_only_message_reports_attempt_and_reason() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "weather".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "denied".to_string(),
                attempted_command: Some(
                    "bravesearch search \"Livermore CA weather tomorrow\" --count 5 --format json"
                        .to_string(),
                ),
                failure_reason: Some("unknown command denied by mode".to_string()),
                refs: vec![],
            }],
        };

        let message = denied_outcomes_only_message(&input).expect("must generate denied message");
        assert!(message.contains("I tried `bravesearch search"));
        assert!(message.contains("unknown command denied by mode"));
        assert!(message.contains("different command path"));
    }

    #[test]
    fn obvious_meta_compose_pattern_catches_user_asks_diagnostic_format() {
        let message = "User asks: “What is the weather?” Diagnostic notes the draft is weak.";
        assert!(obvious_meta_compose_pattern(message));
    }

    #[test]
    fn obvious_meta_compose_pattern_catches_evidence_summary_diagnostic_format() {
        let message = "The evidence summary explicitly says no relevant evidence was found.";
        assert!(obvious_meta_compose_pattern(message));
    }

    #[test]
    fn strip_unexpanded_render_tokens_removes_ref_markers() {
        let message = "answer [[ref:artifact-1]] and [[summary:artifact-2]] done";
        assert_eq!(strip_unexpanded_render_tokens(message), "answer  and  done");
    }

    #[test]
    fn has_repeated_bash_outcome_detects_duplicate_mainline_command() {
        let tool_results = vec![
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![],
                }),
            },
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![],
                }),
            },
        ];
        assert!(has_repeated_bash_outcome(&tool_results));
    }

    #[test]
    fn has_repeated_bash_outcome_ignores_different_commands() {
        let tool_results = vec![
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
                disposition: RuntimeDisposition::Denied {
                    reason: "unknown command denied by mode".to_string(),
                },
            },
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"y\" --count 5 --output json".to_string(),
                disposition: RuntimeDisposition::Denied {
                    reason: "unknown command denied by mode".to_string(),
                },
            },
        ];
        assert!(!has_repeated_bash_outcome(&tool_results));
    }

    #[test]
    fn has_repeated_bash_outcome_detects_case_only_query_variants() {
        let tool_results = vec![
            PlannerToolResult::Bash {
                command:
                    "bravesearch search --query \"weather Livermore CA tomorrow\" --count 1 --output json"
                        .to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-1".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/a".to_string(),
                        byte_count: 2830,
                        line_count: 1,
                    }],
                }),
            },
            PlannerToolResult::Bash {
                command:
                    "bravesearch search --query \"weather Livermore ca tomorrow\" --count 1 --output json"
                        .to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-2".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/b".to_string(),
                        byte_count: 2830,
                        line_count: 1,
                    }],
                }),
            },
        ];

        assert!(has_repeated_bash_outcome(&tool_results));
    }

    #[test]
    fn has_repeated_bash_outcome_ignores_changed_artifact_signature() {
        let tool_results = vec![
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 1 --output json".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-1".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/a".to_string(),
                        byte_count: 100,
                        line_count: 1,
                    }],
                }),
            },
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 1 --output json".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-2".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/b".to_string(),
                        byte_count: 101,
                        line_count: 1,
                    }],
                }),
            },
        ];

        assert!(!has_repeated_bash_outcome(&tool_results));
    }

    #[test]
    fn planner_policy_feedback_includes_missing_connect_denials() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "curl -sS \"https://wttr.in/Livermore,CA?format=j1\"".to_string(),
            disposition: RuntimeDisposition::Denied {
                reason: "missing capability Net:Connect:https://wttr.in/Livermore,CA".to_string(),
            },
        }];

        let feedback = planner_policy_feedback(&tool_results).expect("feedback expected");
        assert!(feedback.contains("https://wttr.in/Livermore,CA"));
        assert!(feedback.contains("markdown.new"));
        assert!(feedback.contains("curl -sS"));
    }

    #[test]
    fn planner_policy_feedback_skips_non_connect_denials() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 1 --output json".to_string(),
            disposition: RuntimeDisposition::Denied {
                reason: "unknown command denied by mode".to_string(),
            },
        }];
        assert!(planner_policy_feedback(&tool_results).is_none());
    }

    #[test]
    fn planner_policy_feedback_includes_markdown_raw_fallback_when_low_signal() {
        let tool_results = vec![PlannerToolResult::Bash {
            command:
                "curl -sS \"https://markdown.new/https://forecast.weather.gov/MapClick.php?lat=37.6819&lon=-121.768\""
                    .to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 81,
                    line_count: 1,
                }],
            }),
        }];

        let feedback = planner_policy_feedback(&tool_results).expect("feedback expected");
        assert!(feedback.contains("markdown proxy fetch returned low/no usable primary content"));
        assert!(feedback.contains(
            "curl -sS \"https://forecast.weather.gov/MapClick.php?lat=37.6819&lon=-121.768\""
        ));
    }

    #[test]
    fn planner_policy_feedback_skips_markdown_raw_fallback_when_primary_content_present() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "curl -sS \"https://markdown.new/https://example.com/article\"".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: MIN_PRIMARY_FETCH_STDOUT_BYTES,
                    line_count: 5,
                }],
            }),
        }];
        assert!(planner_policy_feedback(&tool_results).is_none());
    }

    #[tokio::test]
    async fn planner_memory_feedback_extracts_sieve_lcm_query_payload() {
        let path = std::env::temp_dir().join(format!(
            "sieve-lcm-query-feedback-{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            serde_json::json!({
                "trusted_hits": [
                    {"excerpt": "You live in Livermore, California."}
                ],
                "untrusted_refs": [
                    {"ref": "lcm:untrusted:summary:sum_abc"}
                ]
            })
            .to_string(),
        )
        .expect("write artifact payload");

        let tool_results = vec![PlannerToolResult::Bash {
            command: "sieve-lcm-cli query --lane both --query \"where do i live\" --json"
                .to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: path.to_string_lossy().to_string(),
                    byte_count: 128,
                    line_count: 1,
                }],
            }),
        }];

        let feedback = planner_memory_feedback(&tool_results)
            .await
            .expect("feedback expected");
        assert!(feedback.contains("trusted excerpt"));
        assert!(feedback.contains("Livermore"));
        assert!(feedback.contains("lcm:untrusted:summary:sum_abc"));

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn planner_memory_feedback_ignores_non_memory_commands() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "curl -sS \"https://example.com\"".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![],
            }),
        }];

        assert!(planner_memory_feedback(&tool_results).await.is_none());
    }

    #[test]
    fn compose_quality_followup_only_triggers_for_missing_evidence() {
        let with_refs = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "weather".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some(
                    "bravesearch search --query \"weather\" --count 5 --output json".to_string(),
                ),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 64,
                    line_count: 1,
                }],
            }],
        };
        let signal = compose_quality_followup_signal(
            Some("REVISE: doesn't directly answer and is missing evidence."),
            &with_refs,
        );
        assert_eq!(signal, Some(PlannerGuidanceSignal::ContinueRefineApproach));

        let generic_signal = compose_quality_followup_signal(
            Some("The response lacks specific weather details."),
            &with_refs,
        );
        assert_eq!(
            generic_signal,
            Some(PlannerGuidanceSignal::ContinueRefineApproach)
        );

        let style_signal = compose_quality_followup_signal(
            Some("REVISE: third-person meta narration."),
            &with_refs,
        );
        assert!(style_signal.is_none());
    }

    #[test]
    fn compose_quality_followup_maps_required_parameter_signal() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "where do i live".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "lcm_expand_query".to_string(),
                outcome: "executed".to_string(),
                attempted_command: None,
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 32,
                    line_count: 1,
                }],
            }],
        };

        let signal = compose_quality_followup_signal(
            Some("REVISE: missing required parameter; please specify."),
            &input,
        );
        assert_eq!(
            signal,
            Some(PlannerGuidanceSignal::ContinueNeedRequiredParameter)
        );
    }

    #[test]
    fn compose_quality_followup_maps_denied_tool_signal() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "weather tomorrow".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "denied".to_string(),
                attempted_command: Some("curl -s 'https://wttr.in'".to_string()),
                failure_reason: Some("unknown command denied by mode".to_string()),
                refs: vec![],
            }],
        };

        let signal = compose_quality_followup_signal(Some("REVISE: tool call was denied."), &input);
        assert_eq!(
            signal,
            Some(PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool)
        );
    }

    #[test]
    fn compose_quality_followup_maps_conflict_signal() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "compare claims".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some("some command".to_string()),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 128,
                    line_count: 4,
                }],
            }],
        };
        let signal = compose_quality_followup_signal(
            Some("REVISE: sources conflict and are inconsistent."),
            &input,
        );
        assert_eq!(
            signal,
            Some(PlannerGuidanceSignal::ContinueResolveSourceConflict)
        );
    }

    #[test]
    fn compose_quality_followup_maps_primary_content_fetch_signal() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "latest status".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some(
                    "bravesearch search --query \"status\" --count 5 --output json".to_string(),
                ),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 64,
                    line_count: 1,
                }],
            }],
        };
        let signal = compose_quality_followup_signal(
            Some("REVISE: discovery/search snippets only; missing primary content."),
            &input,
        );
        assert_eq!(
            signal,
            Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch)
        );
    }

    #[test]
    fn compose_quality_followup_maps_url_extraction_signal() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "summarize".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some("curl -sS https://example.com".to_string()),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 64,
                    line_count: 1,
                }],
            }],
        };
        let signal = compose_quality_followup_signal(
            Some("REVISE: need URL extraction from fetched content before next step."),
            &input,
        );
        assert_eq!(
            signal,
            Some(PlannerGuidanceSignal::ContinueNeedUrlExtraction)
        );
    }

    #[test]
    fn guidance_continue_decision_auto_extends_step_limit() {
        let (should_continue, next_limit, auto_extended) = guidance_continue_decision(
            PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
            0,
            3,
            3,
            6,
        );
        assert!(should_continue);
        assert_eq!(next_limit, 4);
        assert!(auto_extended);
    }

    #[test]
    fn guidance_continue_decision_honors_hard_limit() {
        let (should_continue, next_limit, auto_extended) = guidance_continue_decision(
            PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
            0,
            6,
            6,
            6,
        );
        assert!(!should_continue);
        assert_eq!(next_limit, 6);
        assert!(!auto_extended);
    }

    #[test]
    fn progress_contract_requires_primary_content_before_fact_ready() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 128,
                    line_count: 1,
                }],
            }),
        }];
        let override_signal = progress_contract_override_signal(
            "What is the current status?",
            PlannerGuidanceSignal::FinalSingleFactReady,
            &tool_results,
        );
        assert_eq!(
            override_signal.map(|(signal, _)| signal),
            Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch)
        );
    }

    #[test]
    fn progress_contract_requires_non_asset_fetch_target() {
        let tool_results = vec![
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-1".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/a".to_string(),
                        byte_count: 128,
                        line_count: 1,
                    }],
                }),
            },
            PlannerToolResult::Bash {
                command: "curl -sS https://imgs.search.brave.com/logo.png".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-2".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/b".to_string(),
                        byte_count: 64,
                        line_count: 1,
                    }],
                }),
            },
        ];
        let override_signal = progress_contract_override_signal(
            "What is the current status?",
            PlannerGuidanceSignal::FinalAnswerReady,
            &tool_results,
        );
        assert_eq!(
            override_signal.map(|(signal, _)| signal),
            Some(PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl)
        );
    }

    #[test]
    fn progress_contract_normalizes_time_bound_continue_to_primary_fetch() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 128,
                    line_count: 1,
                }],
            }),
        }];
        let override_signal = progress_contract_override_signal(
            "What is the current status?",
            PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence,
            &tool_results,
        );
        assert_eq!(
            override_signal.map(|(signal, _)| signal),
            Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch)
        );
    }

    #[test]
    fn progress_contract_does_not_override_hard_stop_signal() {
        let tool_results = vec![PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 128,
                    line_count: 1,
                }],
            }),
        }];
        let override_signal = progress_contract_override_signal(
            "What is the current status?",
            PlannerGuidanceSignal::StopNoAllowedToolCanSatisfyTask,
            &tool_results,
        );
        assert!(override_signal.is_none());
    }

    #[test]
    fn progress_contract_requests_higher_quality_when_fetch_output_is_too_small() {
        let tool_results = vec![
            PlannerToolResult::Bash {
                command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-1".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/a".to_string(),
                        byte_count: 256,
                        line_count: 1,
                    }],
                }),
            },
            PlannerToolResult::Bash {
                command: "curl -sS \"https://markdown.new/https://example.com/path?x=1\""
                    .to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-2".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/b".to_string(),
                        byte_count: 81,
                        line_count: 1,
                    }],
                }),
            },
        ];
        let override_signal = progress_contract_override_signal(
            "What is the weather today?",
            PlannerGuidanceSignal::FinalAnswerReady,
            &tool_results,
        );
        assert_eq!(
            override_signal.map(|(signal, _)| signal),
            Some(PlannerGuidanceSignal::ContinueNeedHigherQualitySource)
        );
    }

    #[test]
    fn enforce_link_policy_appends_plain_urls_when_link_claim_has_no_url() {
        let message = "For more information, visit the provided link.".to_string();
        let enforced = enforce_link_policy(
            message,
            &[
                "https://example.com/a".to_string(),
                "https://example.com/b".to_string(),
            ],
            "please include sources and links",
        );
        assert!(enforced.contains("https://example.com/a"));
        assert!(enforced.contains("https://example.com/b"));
        assert!(enforced.contains("provided link"));
    }

    #[test]
    fn enforce_link_policy_strips_link_claim_without_available_urls() {
        let message = "Top result is ready. Visit the provided link for details.".to_string();
        let enforced = enforce_link_policy(message, &[], "just answer briefly");
        assert!(!enforced.to_ascii_lowercase().contains("provided link"));
    }

    #[test]
    fn enforce_link_policy_does_not_append_urls_when_sources_not_requested() {
        let message = "For more information, visit the provided link.".to_string();
        let enforced = enforce_link_policy(
            message,
            &["https://example.com/a".to_string()],
            "just answer",
        );
        assert!(!enforced.contains("https://example.com/a"));
        assert!(!enforced.to_ascii_lowercase().contains("provided link"));
    }

    #[test]
    fn filter_non_asset_urls_removes_asset_links() {
        let filtered = filter_non_asset_urls(vec![
            "https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125"
                .to_string(),
            "https://imgs.search.brave.com/fs6uyhM5xA6gctiAKJTHhWtpR2YRWceKfG_9aqjmfRs/rs:fit:32:32:1:0/g:ce/a.png"
                .to_string(),
            "https://example.com/favicon.ico".to_string(),
        ]);
        assert_eq!(
            filtered,
            vec![
                "https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125"
                    .to_string()
            ]
        );
    }

    #[test]
    fn strip_asset_urls_from_message_removes_asset_plain_urls() {
        let message = "Useful: https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125\nhttps://imgs.search.brave.com/example.png";
        let stripped = strip_asset_urls_from_message(message);
        assert!(stripped
            .contains("https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125"));
        assert!(!stripped.contains("https://imgs.search.brave.com/example.png"));
    }

    #[test]
    fn extract_plain_urls_from_text_handles_jsonish_tokens() {
        let text = "{\"url\":\"https://weather.com/weather/tenday/l/Dublin\",\"other\":\"x\"}";
        let urls = extract_plain_urls_from_text(text);
        assert_eq!(
            urls,
            vec!["https://weather.com/weather/tenday/l/Dublin".to_string()]
        );
    }

    #[test]
    fn source_and_detail_request_detection() {
        assert!(user_requested_sources("please include links to sources"));
        assert!(user_requested_sources("cite references"));
        assert!(!user_requested_sources("just tell me the answer"));

        assert!(user_requested_detailed_output(
            "give a detailed explanation"
        ));
        assert!(user_requested_detailed_output("step by step please"));
        assert!(!user_requested_detailed_output("just short answer"));
    }

    #[test]
    fn concise_style_diagnostic_flags_unsolicited_source_dump() {
        let message = "Answer first. https://a.example https://b.example";
        let diagnostic = concise_style_diagnostic(message, "What is the answer?");
        assert!(diagnostic.is_some());
        let detail_ok = concise_style_diagnostic(message, "Give sources and links");
        assert!(detail_ok.is_none());
    }

    #[test]
    fn load_dotenv_from_path_missing_file_is_noop() {
        let _guard = env_test_lock()
            .lock()
            .expect("dotenv env test lock poisoned");
        let path = std::env::temp_dir().join(format!(
            "sieve-app-missing-env-{}-{}",
            std::process::id(),
            now_ms()
        ));
        assert!(load_dotenv_from_path(&path).is_ok());
    }

    #[test]
    fn load_dotenv_from_path_sets_values() {
        let _guard = env_test_lock()
            .lock()
            .expect("dotenv env test lock poisoned");
        let tmp_dir = std::env::temp_dir().join(format!(
            "sieve-app-dotenv-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&tmp_dir).expect("create temp test dir");
        let env_path = tmp_dir.join(".env");
        let key = format!("SIEVE_APP_DOTENV_TEST_{}_{}", std::process::id(), now_ms());
        std::env::remove_var(&key);
        fs::write(&env_path, format!("{key}=from_dotenv\n")).expect("write dotenv file");

        load_dotenv_from_path(&env_path).expect("load dotenv from path");
        let loaded = std::env::var(&key).expect("dotenv variable must be set");
        assert_eq!(loaded, "from_dotenv");

        std::env::remove_var(&key);
        let _ = fs::remove_file(&env_path);
        let _ = fs::remove_dir(&tmp_dir);
    }
}
