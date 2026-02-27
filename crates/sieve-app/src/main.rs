#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde::Serialize;
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge, TelegramPrompt,
};
use sieve_llm::{
    OpenAiPlannerModel, OpenAiResponseModel, OpenAiSummaryModel, ResponseModel,
    ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput, SummaryModel, SummaryRequest,
};
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{
    ApprovalBusError, EventLogError, InProcessApprovalBus, JsonlRuntimeEventLog, MainlineArtifact,
    MainlineArtifactKind, MainlineRunError, MainlineRunReport, MainlineRunRequest, MainlineRunner,
    PlannerRunRequest, PlannerRunResult, PlannerToolResult, RuntimeDeps, RuntimeDisposition,
    RuntimeEventLog, RuntimeOrchestrator, SystemClock as RuntimeClock, WebSearchDisposition,
    WebSearchError, WebSearchRunner,
};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{
    ApprovalResolvedEvent, AssistantMessageEvent, BraveSearchRequest, BraveSearchResponse,
    BraveSearchResult, Integrity, InteractionModality, ModalityContract, ModalityOverrideReason,
    RunId, RuntimeEvent, UncertainMode, UnknownMode,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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
    brave_api_key: Option<String>,
    brave_api_base: String,
    audio_stt_cmd: Option<String>,
    audio_tts_cmd: Option<String>,
    image_ocr_cmd: Option<String>,
    unknown_mode: UnknownMode,
    uncertain_mode: UncertainMode,
    max_concurrent_turns: usize,
}

const DEFAULT_POLICY_PATH: &str = "docs/policy/baseline-policy.toml";
const DEFAULT_SIEVE_DIR_NAME: &str = ".sieve";
const DEFAULT_BRAVE_API_BASE: &str = "https://api.search.brave.com/res/v1/web/search";

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
                .unwrap_or_else(|_| "bash,endorse,declassify,brave_search".to_string()),
        );
        if allowed_tools.is_empty() {
            return Err("SIEVE_ALLOWED_TOOLS must include at least one tool".to_string());
        }
        let brave_api_key = optional_env("BRAVE_API_KEY");
        let brave_api_base = env::var("SIEVE_BRAVE_API_BASE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_BRAVE_API_BASE.to_string());
        let audio_stt_cmd = optional_env("SIEVE_AUDIO_STT_CMD");
        let audio_tts_cmd = optional_env("SIEVE_AUDIO_TTS_CMD");
        let image_ocr_cmd = optional_env("SIEVE_IMAGE_OCR_CMD");
        let max_concurrent_turns = parse_usize_env("SIEVE_MAX_CONCURRENT_TURNS", 4)?;
        if max_concurrent_turns == 0 {
            return Err("SIEVE_MAX_CONCURRENT_TURNS must be >= 1".to_string());
        }

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
            brave_api_key,
            brave_api_base,
            audio_stt_cmd,
            audio_tts_cmd,
            image_ocr_cmd,
            unknown_mode: parse_unknown_mode(env::var("SIEVE_UNKNOWN_MODE").ok())?,
            uncertain_mode: parse_uncertain_mode(env::var("SIEVE_UNCERTAIN_MODE").ok())?,
            max_concurrent_turns,
        })
    }
}

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("missing required environment variable `{key}`"))
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
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

struct AppBraveSearchRunner {
    api_key: Option<String>,
    api_base: String,
}

impl AppBraveSearchRunner {
    fn new(api_key: Option<String>, api_base: String) -> Self {
        Self { api_key, api_base }
    }
}

#[async_trait]
impl WebSearchRunner for AppBraveSearchRunner {
    fn connect_scope(&self) -> String {
        self.api_base.clone()
    }

    async fn search(
        &self,
        request: BraveSearchRequest,
    ) -> Result<BraveSearchResponse, WebSearchError> {
        let api_key = self.api_key.clone().ok_or_else(|| {
            WebSearchError::Exec("BRAVE_API_KEY is required to use brave_search tool".to_string())
        })?;

        let output = TokioCommand::new("curl")
            .arg("-sS")
            .arg("--fail")
            .arg("--get")
            .arg("-H")
            .arg("Accept: application/json")
            .arg("-H")
            .arg(format!("X-Subscription-Token: {api_key}"))
            .arg("--data-urlencode")
            .arg(format!("q={}", request.query))
            .arg("--data-urlencode")
            .arg(format!("count={}", request.count))
            .arg(&self.api_base)
            .output()
            .await
            .map_err(|err| WebSearchError::Exec(format!("failed to spawn curl: {err}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(WebSearchError::Exec(if stderr.is_empty() {
                "curl failed calling Brave API".to_string()
            } else {
                format!("curl failed calling Brave API: {stderr}")
            }));
        }

        decode_brave_search_response(&output.stdout, request.query)
    }
}

fn decode_brave_search_response(
    bytes: &[u8],
    query: String,
) -> Result<BraveSearchResponse, WebSearchError> {
    let payload: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|err| WebSearchError::Exec(format!("invalid Brave API JSON response: {err}")))?;
    let mut results = Vec::new();
    if let Some(items) = payload
        .get("web")
        .and_then(|web| web.get("results"))
        .and_then(serde_json::Value::as_array)
    {
        for item in items {
            let Some(url) = item.get("url").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if url.trim().is_empty() {
                continue;
            }
            let title = item
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(url)
                .to_string();
            let description = item
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            results.push(BraveSearchResult {
                title,
                url: url.to_string(),
                description,
            });
        }
    }

    Ok(BraveSearchResponse { query, results })
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
}

fn non_empty_output_ref_ids(input: &ResponseTurnInput) -> BTreeSet<String> {
    input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|ref_metadata| {
            (ref_metadata.kind == "stdout" || ref_metadata.kind == "stderr")
                && ref_metadata.byte_count > 0
        })
        .map(|ref_metadata| ref_metadata.ref_id.clone())
        .collect()
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
            command: _,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: format!("executed mainline (exit_code={:?})", report.exit_code),
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
                outcome: format!("denied ({reason})"),
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
        PlannerToolResult::BraveSearch {
            request,
            disposition,
        } => match disposition {
            WebSearchDisposition::Executed(response) => {
                let ref_id = format!("web-search:{}-{}", request.count, render_refs.len() + 1);
                let encoded = serde_json::to_string_pretty(response).unwrap_or_else(|_| {
                    "{\"error\":\"failed to encode brave search response\"}".to_string()
                });
                let bytes = encoded.as_bytes();
                render_refs.insert(
                    ref_id.clone(),
                    RenderRef::Literal {
                        value: encoded.clone(),
                    },
                );
                ResponseToolOutcome {
                    tool_name: "brave_search".to_string(),
                    outcome: format!(
                        "brave search executed (query={:?}, results={})",
                        request.query,
                        response.results.len()
                    ),
                    refs: vec![ResponseRefMetadata {
                        ref_id,
                        kind: "web_search_results_json".to_string(),
                        byte_count: bytes.len() as u64,
                        line_count: count_newlines(bytes),
                    }],
                }
            }
            WebSearchDisposition::Denied { reason } => ResponseToolOutcome {
                tool_name: "brave_search".to_string(),
                outcome: format!("brave search denied ({reason})"),
                refs: Vec::new(),
            },
        },
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

fn shell_escape_single_quoted(value: &str) -> String {
    let mut escaped = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            escaped.push_str("'\\''");
        } else {
            escaped.push(ch);
        }
    }
    escaped.push('\'');
    escaped
}

fn render_shell_template(template: &str, replacements: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in replacements {
        let placeholder = format!("{{{{{key}}}}}");
        out = out.replace(&placeholder, &shell_escape_single_quoted(value));
    }
    out
}

fn command_error_from_output(context: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{context} failed")
    } else {
        format!("{context} failed: {stderr}")
    }
}

async fn run_shell_template_capture_stdout(
    template: &str,
    replacements: &[(&str, String)],
    context: &str,
) -> Result<String, String> {
    let script = render_shell_template(template, replacements);
    let output = TokioCommand::new("bash")
        .arg("-lc")
        .arg(script)
        .output()
        .await
        .map_err(|err| format!("{context} spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output(context, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn run_shell_template(
    template: &str,
    replacements: &[(&str, String)],
    context: &str,
) -> Result<(), String> {
    let script = render_shell_template(template, replacements);
    let output = TokioCommand::new("bash")
        .arg("-lc")
        .arg(script)
        .output()
        .await
        .map_err(|err| format!("{context} spawn failed: {err}"))?;
    if !output.status.success() {
        return Err(command_error_from_output(context, &output));
    }
    Ok(())
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
    let stt_cmd = cfg
        .audio_stt_cmd
        .clone()
        .ok_or_else(|| "audio input requires SIEVE_AUDIO_STT_CMD".to_string())?;
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

    let transcript = run_shell_template_capture_stdout(
        &stt_cmd,
        &[
            ("input_path", input_path.to_string_lossy().to_string()),
            ("run_id", run_id.0.clone()),
        ],
        "audio stt command",
    )
    .await?;
    let transcript = transcript.trim().to_string();
    if transcript.is_empty() {
        return Err("audio stt command produced empty transcript".to_string());
    }
    Ok(transcript)
}

async fn extract_image_prompt(
    cfg: &AppConfig,
    run_id: &RunId,
    file_id: &str,
) -> Result<String, String> {
    let ocr_cmd = cfg
        .image_ocr_cmd
        .clone()
        .ok_or_else(|| "image input requires SIEVE_IMAGE_OCR_CMD".to_string())?;
    let file_path = fetch_telegram_file_path(&cfg.telegram_bot_token, file_id).await?;
    let media_dir = cfg.sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let ext = std::path::Path::new(&file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .unwrap_or("jpg");
    let input_path = media_dir.join(format!("image-input.{ext}"));
    download_telegram_file(&cfg.telegram_bot_token, &file_path, &input_path).await?;

    let extracted = run_shell_template_capture_stdout(
        &ocr_cmd,
        &[
            ("input_path", input_path.to_string_lossy().to_string()),
            ("run_id", run_id.0.clone()),
        ],
        "image ocr command",
    )
    .await?;
    let extracted = extracted.trim().to_string();
    if extracted.is_empty() {
        return Err("image ocr command produced empty output".to_string());
    }
    Ok(extracted)
}

async fn synthesize_audio_reply(
    cfg: &AppConfig,
    run_id: &RunId,
    assistant_message: &str,
) -> Result<PathBuf, String> {
    let tts_cmd = cfg
        .audio_tts_cmd
        .clone()
        .ok_or_else(|| "audio reply requires SIEVE_AUDIO_TTS_CMD".to_string())?;
    let media_dir = cfg.sieve_home.join("media").join(&run_id.0);
    tokio::fs::create_dir_all(&media_dir)
        .await
        .map_err(|err| format!("failed to create media dir: {err}"))?;
    let text_path = media_dir.join("tts-input.txt");
    let output_path = media_dir.join("tts-output.ogg");
    tokio::fs::write(&text_path, assistant_message)
        .await
        .map_err(|err| format!("failed to write tts input text: {err}"))?;
    run_shell_template(
        &tts_cmd,
        &[
            ("text_path", text_path.to_string_lossy().to_string()),
            ("output_path", output_path.to_string_lossy().to_string()),
            ("run_id", run_id.0.clone()),
        ],
        "audio tts command",
    )
    .await?;
    let metadata = tokio::fs::metadata(&output_path)
        .await
        .map_err(|err| format!("audio tts output missing: {err}"))?;
    if metadata.len() == 0 {
        return Err("audio tts output file is empty".to_string());
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

async fn run_turn(
    runtime: &RuntimeOrchestrator,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
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
            Some(file_id) => match extract_image_prompt(cfg, &run_id, file_id).await {
                Ok(extracted) => (extracted, None),
                Err(err) => (
                    String::new(),
                    Some(format!("image input unavailable: {err}")),
                ),
            },
            None => (
                String::new(),
                Some("image input missing media file id".to_string()),
            ),
        },
    };
    if let Some(error_message) = input_error {
        println!("{}: {}", run_id.0, error_message);
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
            .await?;
        return Ok(());
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::User,
            trusted_user_message.clone(),
            now_ms(),
        ))
        .await?;

    let result = match runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: run_id.clone(),
            cwd: cfg.runtime_cwd.clone(),
            user_message: trusted_user_message.clone(),
            allowed_tools: cfg.allowed_tools.clone(),
            previous_events: event_log.snapshot(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: cfg.unknown_mode,
            uncertain_mode: cfg.uncertain_mode,
        })
        .await
    {
        Ok(result) => result,
        Err(err) => {
            if let Err(log_err) = event_log
                .append_conversation(ConversationLogRecord::new(
                    run_id.clone(),
                    ConversationRole::Assistant,
                    format!("error: {err}"),
                    now_ms(),
                ))
                .await
            {
                eprintln!("failed to append assistant error conversation log: {log_err}");
            }
            return Err(err.into());
        }
    };

    let (mut response_input, render_refs) =
        build_response_turn_input(&run_id, &trusted_user_message, &result);
    let mut response_output = match response_model
        .write_turn_response(response_input.clone())
        .await
    {
        Ok(response) => response,
        Err(err) => {
            if let Err(log_err) = event_log
                .append_conversation(ConversationLogRecord::new(
                    run_id.clone(),
                    ConversationRole::Assistant,
                    format!("error: {err}"),
                    now_ms(),
                ))
                .await
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
        let diagnostics = "Non-empty stdout/stderr refs exist. Include at least one output token directly in `message` using [[ref:<id>]] or [[summary:<id>]], and list the same id in referenced_ref_ids or summarized_ref_ids.";
        response_input.planner_thoughts = Some(match response_input.planner_thoughts.take() {
            Some(existing) if !existing.trim().is_empty() => format!("{existing}\n{diagnostics}"),
            _ => diagnostics.to_string(),
        });

        response_output = match response_model
            .write_turn_response(response_input.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(log_err) = event_log
                    .append_conversation(ConversationLogRecord::new(
                        run_id.clone(),
                        ConversationRole::Assistant,
                        format!("error: {err}"),
                        now_ms(),
                    ))
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

    let assistant_message = render_assistant_message(
        &response_output.message,
        &response_output.referenced_ref_ids,
        &response_output.summarized_ref_ids,
        &render_refs,
        summary_model,
        &run_id,
    )
    .await;
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
    response_model: Arc<dyn ResponseModel>,
    summary_model: Arc<dyn SummaryModel>,
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
        let response_model = response_model.clone();
        let summary_model = summary_model.clone();
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
                response_model.as_ref(),
                summary_model.as_ref(),
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

    let cfg =
        AppConfig::from_env().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let policy_toml = fs::read_to_string(&cfg.policy_path)?;
    let policy = TomlPolicyEngine::from_toml_str(&policy_toml)?;

    let planner = OpenAiPlannerModel::from_env()?;
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
        web_search: Arc::new(AppBraveSearchRunner::new(
            cfg.brave_api_key.clone(),
            cfg.brave_api_base.clone(),
        )),
        planner: Arc::new(planner),
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    }));

    if single_command_mode {
        run_turn(
            &runtime,
            response_model.as_ref(),
            summary_model.as_ref(),
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
            response_model.clone(),
            summary_model.clone(),
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
    use sieve_llm::LlmError;
    use sieve_runtime::ApprovalBus;
    use sieve_types::{
        ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, CommandSegment, PolicyDecision,
        PolicyDecisionKind, PolicyEvaluatedEvent, Resource,
    };
    use std::sync::{Mutex as StdMutex, OnceLock};

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
            Ok(format!(
                "summary(bytes={},lines={})",
                request.byte_count, request.line_count
            ))
        }
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
    fn render_shell_template_quotes_replacements() {
        let rendered = render_shell_template(
            "tool --input {{input_path}} --run {{run_id}}",
            &[
                ("input_path", "/tmp/it's ok.wav".to_string()),
                ("run_id", "run-1".to_string()),
            ],
        );
        assert!(rendered.contains("--input '/tmp/it'\\''s ok.wav'"));
        assert!(rendered.contains("--run 'run-1'"));
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
    fn response_has_visible_selected_output_requires_message_token() {
        let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "show output".to_string(),
            planner_thoughts: None,
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
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
