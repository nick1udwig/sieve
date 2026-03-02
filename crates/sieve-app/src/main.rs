#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde::Serialize;
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
    ApprovalResolvedEvent, AssistantMessageEvent, Integrity, InteractionModality, ModalityContract,
    ModalityOverrideReason, PlannerGuidanceFrame, PlannerGuidanceInput, PlannerGuidanceSignal,
    RunId, RuntimeEvent, UncertainMode, UnknownMode,
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
    audio_stt_cmd: Option<String>,
    audio_tts_cmd: Option<String>,
    image_ocr_cmd: Option<String>,
    unknown_mode: UnknownMode,
    uncertain_mode: UncertainMode,
    max_concurrent_turns: usize,
    max_planner_steps: usize,
}

const DEFAULT_POLICY_PATH: &str = "docs/policy/baseline-policy.toml";
const DEFAULT_SIEVE_DIR_NAME: &str = ".sieve";

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
        let audio_stt_cmd = optional_env("SIEVE_AUDIO_STT_CMD");
        let audio_tts_cmd = optional_env("SIEVE_AUDIO_TTS_CMD");
        let image_ocr_cmd = optional_env("SIEVE_IMAGE_OCR_CMD");
        let max_concurrent_turns = parse_usize_env("SIEVE_MAX_CONCURRENT_TURNS", 4)?;
        if max_concurrent_turns == 0 {
            return Err("SIEVE_MAX_CONCURRENT_TURNS must be >= 1".to_string());
        }
        let max_planner_steps = parse_usize_env("SIEVE_MAX_PLANNER_STEPS", 3)?;
        if max_planner_steps == 0 {
            return Err("SIEVE_MAX_PLANNER_STEPS must be >= 1".to_string());
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
            audio_stt_cmd,
            audio_tts_cmd,
            image_ocr_cmd,
            unknown_mode: parse_unknown_mode(env::var("SIEVE_UNKNOWN_MODE").ok())?,
            uncertain_mode: parse_uncertain_mode(env::var("SIEVE_UNCERTAIN_MODE").ok())?,
            max_concurrent_turns,
            max_planner_steps,
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

fn summarize_observed_tool_result(result: &PlannerToolResult) -> serde_json::Value {
    match result {
        PlannerToolResult::Bash {
            command,
            disposition,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
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
                    "disposition": "execute_mainline",
                    "exit_code": report.exit_code,
                    "artifact_count": report.artifacts.len(),
                    "stdout_bytes": stdout_bytes,
                    "stderr_bytes": stderr_bytes
                })
            }
            RuntimeDisposition::ExecuteQuarantine(report) => serde_json::json!({
                "tool": "bash",
                "command_len": command.len(),
                "disposition": "execute_quarantine",
                "exit_code": report.exit_code,
                "trace_path_present": !report.trace_path.trim().is_empty(),
                "stdout_path_present": report.stdout_path.as_deref().is_some(),
                "stderr_path_present": report.stderr_path.as_deref().is_some()
            }),
            RuntimeDisposition::Denied { reason } => serde_json::json!({
                "tool": "bash",
                "command_len": command.len(),
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

fn build_guidance_prompt(
    trusted_user_message: &str,
    step_index: usize,
    max_steps: usize,
    step_results: &[PlannerToolResult],
    total_results: usize,
) -> String {
    let observed_results: Vec<serde_json::Value> = step_results
        .iter()
        .map(summarize_observed_tool_result)
        .collect();
    serde_json::json!({
        "task": "planner_act_observe",
        "trusted_user_message": trusted_user_message,
        "step_index": step_index,
        "max_steps": max_steps,
        "step_tool_result_count": step_results.len(),
        "total_tool_result_count": total_results,
        "observed_step_results": observed_results,
        "instruction": "Return numeric guidance code: continue only if more tool actions are still needed; otherwise return final or stop."
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
    )
}

fn normalize_chat_probe(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .replace(['?', '!', '.', ',', ';', ':'], " ")
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

fn is_chat_only_prompt(input: &str) -> bool {
    let normalized = normalize_chat_probe(input);
    if normalized.is_empty() {
        return false;
    }

    matches!(
        normalized.as_str(),
        "hi" | "hello"
            | "hey"
            | "yo"
            | "hi how are you"
            | "hello how are you"
            | "hey how are you"
            | "how are you"
            | "how are you today"
            | "hi can you hear me"
            | "hi can you hear me how are you"
            | "hello can you hear me how are you"
            | "hey can you hear me how are you"
            | "can you hear me"
            | "who are you"
            | "whats your name"
            | "what's your name"
            | "what is your name"
            | "what s your name"
            | "thanks"
            | "thank you"
            | "good morning"
            | "good afternoon"
            | "good evening"
    )
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

fn extract_plain_urls_from_text(message: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for token in message.split_whitespace() {
        let trimmed = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '"' | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | ','
                    | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
            )
        });
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            urls.push(trimmed.to_string());
        }
    }
    dedupe_preserve_order(urls)
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

fn enforce_link_policy(message: String, source_urls: &[String]) -> String {
    if !mentions_linkish_text(&message) || contains_plain_url(&message) {
        return message;
    }
    if !source_urls.is_empty() {
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

fn obvious_meta_compose_pattern(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    normalized.starts_with("the assistant ")
        || normalized.starts_with("assistant is ")
        || normalized.contains("the user has")
        || normalized.contains("user has asked")
        || normalized.contains("the assistant is ready to help")
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

fn compose_quality_requires_retry(
    composed_message: &str,
    quality_gate: Option<&str>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    let gate = quality_gate.unwrap_or("").trim();
    if gate.is_empty() {
        return None;
    }
    let lower = gate.to_ascii_lowercase();
    if lower.starts_with("pass") || (lower.contains("pass") && !lower.contains("revise")) {
        None
    } else {
        Some(gate.to_string())
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
}

fn compose_quality_followup_signal(
    quality_gate: Option<&str>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let gate = quality_gate.unwrap_or("").trim();
    if gate.is_empty() {
        return None;
    }
    let lower = gate.to_ascii_lowercase();
    if lower.starts_with("pass") || (lower.contains("pass") && !lower.contains("revise")) {
        return None;
    }

    let has_non_empty_refs = response_input
        .tool_outcomes
        .iter()
        .any(|outcome| outcome.refs.iter().any(|metadata| metadata.byte_count > 0));
    if !has_non_empty_refs {
        return None;
    }

    let is_style_only = lower.contains("third-person")
        || lower.contains("third person")
        || lower.contains("meta narration")
        || lower.contains("tone");
    if is_style_only {
        return None;
    }

    let missing_evidence = lower.contains("insufficient")
        || lower.contains("not enough")
        || lower.contains("missing")
        || lower.contains("doesn't directly answer")
        || lower.contains("does not directly answer")
        || lower.contains("doesn’t directly answer")
        || lower.contains("can't")
        || lower.contains("cannot")
        || lower.contains("can’t")
        || lower.contains("no actual")
        || lower.contains("real-time")
        || lower.contains("right now");
    if missing_evidence {
        return Some(PlannerGuidanceSignal::ContinueFetchAdditionalSource);
    }

    None
}

async fn write_compose_audit_artifacts(
    sieve_home: &Path,
    run_id: &RunId,
    attempts: &[serde_json::Value],
    final_message: &str,
    output_ref_ids: &[String],
    source_urls: &[String],
    quality_gate: Option<&str>,
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
        "planner_followup_signal_code": planner_followup_signal.map(PlannerGuidanceSignal::code),
    });
    append_jsonl_record(&logs_path, &record).await
}

async fn compose_assistant_message(
    summary_model: &dyn SummaryModel,
    sieve_home: &Path,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    draft_message: String,
) -> ComposeAssistantOutcome {
    let output_ref_ids: Vec<String> = non_empty_output_ref_ids(response_input)
        .into_iter()
        .collect();
    let source_urls = dedupe_preserve_order(extract_plain_urls_from_text(&draft_message));
    let tool_outcomes: Vec<serde_json::Value> = response_input
        .tool_outcomes
        .iter()
        .map(|outcome| {
            serde_json::json!({
                "tool_name": outcome.tool_name,
                "outcome": outcome.outcome,
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
        "assistant_draft_message": draft_message,
        "planner_thoughts": response_input.planner_thoughts.clone(),
        "tool_outcomes": tool_outcomes,
        "output_ref_ids": output_ref_ids.clone(),
        "available_plain_urls": source_urls.clone(),
    });
    attempt_payloads.push(payload.clone());

    let first_composed = summarize_with_ref_id(
        summary_model,
        run_id,
        &format!("assistant-compose:{}", run_id.0),
        &payload,
    )
    .await
    .unwrap_or_else(|| {
        payload
            .get("assistant_draft_message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });

    let quality_payload = serde_json::json!({
        "task": "compose_quality_gate",
        "trusted_user_message": trusted_user_message,
        "composed_message": first_composed,
    });
    let first_quality_gate = summarize_with_ref_id(
        summary_model,
        run_id,
        &format!("assistant-compose-quality:{}", run_id.0),
        &quality_payload,
    )
    .await;

    let mut composed = quality_payload
        .get("composed_message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut did_retry = false;
    if let Some(diagnostic) =
        compose_quality_requires_retry(&composed, first_quality_gate.as_deref())
    {
        did_retry = true;
        let retry_payload = serde_json::json!({
            "task": "compose_user_reply",
            "trusted_user_message": trusted_user_message,
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
            "compose_diagnostic": diagnostic,
            "previous_composed_message": composed,
        });
        attempt_payloads.push(retry_payload.clone());
        composed = summarize_with_ref_id(
            summary_model,
            run_id,
            &format!("assistant-compose-retry:{}", run_id.0),
            &retry_payload,
        )
        .await
        .unwrap_or_else(|| {
            retry_payload
                .get("previous_composed_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
    }

    let quality_gate = if did_retry {
        let final_quality_payload = serde_json::json!({
            "task": "compose_quality_gate",
            "trusted_user_message": trusted_user_message,
            "composed_message": composed,
        });
        summarize_with_ref_id(
            summary_model,
            run_id,
            &format!("assistant-compose-quality-final:{}", run_id.0),
            &final_quality_payload,
        )
        .await
    } else {
        first_quality_gate.clone()
    };

    let planner_followup_signal =
        compose_quality_followup_signal(quality_gate.as_deref(), response_input);
    let planner_decision = planner_followup_signal
        .map(ComposePlannerDecision::Continue)
        .unwrap_or(ComposePlannerDecision::Finalize);

    let composed = enforce_link_policy(composed, &source_urls);
    if let Err(err) = write_compose_audit_artifacts(
        sieve_home,
        run_id,
        &attempt_payloads,
        &composed,
        &output_ref_ids,
        &source_urls,
        quality_gate.as_deref(),
        planner_followup_signal,
    )
    .await
    {
        eprintln!("compose audit write failed for {}: {}", run_id.0, err);
    }
    ComposeAssistantOutcome {
        message: composed,
        quality_gate,
        planner_decision,
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
        emit_assistant_error_message(event_log, &run_id, error_message).await?;
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

    let mut aggregated_result = PlannerRunResult {
        thoughts: None,
        tool_results: Vec::new(),
    };
    let chat_only_turn =
        input_modality == InteractionModality::Text && is_chat_only_prompt(&trusted_user_message);
    if chat_only_turn {
        aggregated_result.thoughts = Some("chat-only turn: no tools needed".to_string());
    }
    let mut planner_guidance: Option<PlannerGuidanceFrame> = None;
    let mut consecutive_empty_steps = 0usize;
    let mut planner_steps_taken = 0usize;
    let mut compose_followup_cycles = 0usize;
    let max_compose_followup_cycles = cfg.max_planner_steps.max(1);
    let planner_step_hard_limit = cfg
        .max_planner_steps
        .saturating_add(max_compose_followup_cycles);
    let mut planner_step_limit = cfg.max_planner_steps.max(1);

    let assistant_message = loop {
        if !chat_only_turn {
            while planner_steps_taken < planner_step_limit {
                let step_number = planner_steps_taken + 1;
                let step_result = match runtime
                    .orchestrate_planner_turn(PlannerRunRequest {
                        run_id: run_id.clone(),
                        cwd: cfg.runtime_cwd.clone(),
                        user_message: trusted_user_message.clone(),
                        allowed_tools: cfg.allowed_tools.clone(),
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
                        if let Err(log_err) = emit_assistant_error_message(
                            event_log,
                            &run_id,
                            format!("error: {err}"),
                        )
                        .await
                        {
                            eprintln!(
                                "failed to append assistant error conversation log: {log_err}"
                            );
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

                let guidance_prompt = build_guidance_prompt(
                    &trusted_user_message,
                    step_number,
                    cfg.max_planner_steps,
                    &step_results,
                    aggregated_result.tool_results.len(),
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
                let should_continue = guidance_requests_continue(signal)
                    && planner_steps_taken < planner_step_limit
                    && consecutive_empty_steps < 2;
                append_turn_controller_event(
                    &cfg.sieve_home,
                    &run_id,
                    "planner_guidance",
                    serde_json::json!({
                        "step_number": step_number,
                        "signal_code": signal.code(),
                        "continue": should_continue,
                        "step_tool_count": step_tool_count,
                        "planner_steps_taken": planner_steps_taken,
                        "planner_step_limit": planner_step_limit,
                        "consecutive_empty_steps": consecutive_empty_steps,
                    }),
                )
                .await;
                planner_guidance = Some(guidance_output.guidance);
                if !should_continue {
                    break;
                }
            }
        }

        let (mut response_input, render_refs) =
            build_response_turn_input(&run_id, &trusted_user_message, &aggregated_result);
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

        let rendered_message = render_assistant_message(
            &response_output.message,
            &response_output.referenced_ref_ids,
            &response_output.summarized_ref_ids,
            &render_refs,
            summary_model,
            &run_id,
        )
        .await;

        let composed = if chat_only_turn {
            ComposeAssistantOutcome {
                message: rendered_message,
                quality_gate: None,
                planner_decision: ComposePlannerDecision::Finalize,
            }
        } else {
            compose_assistant_message(
                summary_model,
                &cfg.sieve_home,
                &run_id,
                &trusted_user_message,
                &response_input,
                rendered_message,
            )
            .await
        };

        if let ComposePlannerDecision::Continue(signal) = composed.planner_decision {
            let can_continue = !chat_only_turn
                && planner_steps_taken < planner_step_hard_limit
                && compose_followup_cycles < max_compose_followup_cycles;
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

    if single_command_mode {
        run_turn(
            &runtime,
            guidance_model.as_ref(),
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
            guidance_model.clone(),
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
    use serde_json::Value;
    use sieve_llm::{GuidanceModel, LlmError, PlannerModel};
    use sieve_runtime::ApprovalBus;
    use sieve_types::{
        ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, CommandSegment, LlmModelConfig,
        LlmProvider, PlannerGuidanceFrame, PlannerGuidanceInput, PlannerGuidanceOutput,
        PlannerGuidanceSignal, PlannerTurnInput, PlannerTurnOutput, PolicyDecision,
        PolicyDecisionKind, PolicyEvaluatedEvent, Resource,
    };
    use std::collections::VecDeque;
    use std::path::Path;
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
            if request.ref_id.starts_with("assistant-compose-quality:") {
                return Ok("PASS".to_string());
            }
            if request.ref_id.starts_with("assistant-compose:")
                || request.ref_id.starts_with("assistant-compose-retry:")
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

    struct QueuedSummaryModel {
        config: LlmModelConfig,
        outputs: StdMutex<VecDeque<Result<String, LlmError>>>,
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
            }
        }
    }

    #[async_trait]
    impl SummaryModel for QueuedSummaryModel {
        fn config(&self) -> &LlmModelConfig {
            &self.config
        }

        async fn summarize_ref(&self, _request: SummaryRequest) -> Result<String, LlmError> {
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
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
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

    enum E2eModelMode {
        Fake {
            planner: Arc<dyn PlannerModel>,
            guidance: Arc<dyn GuidanceModel>,
            response: Arc<dyn ResponseModel>,
            summary: Arc<dyn SummaryModel>,
        },
        RealOpenAi,
    }

    struct AppE2eHarness {
        runtime: Arc<RuntimeOrchestrator>,
        guidance_model: Arc<dyn GuidanceModel>,
        response_model: Arc<dyn ResponseModel>,
        summary_model: Arc<dyn SummaryModel>,
        event_log: Arc<FanoutRuntimeEventLog>,
        cfg: AppConfig,
        next_run_index: AtomicU64,
        root: PathBuf,
        _telegram_event_rx: Receiver<TelegramLoopEvent>,
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
            let cfg = AppConfig {
                telegram_bot_token: "test-token".to_string(),
                telegram_chat_id: 42,
                telegram_poll_timeout_secs: 1,
                telegram_allowed_sender_user_ids: None,
                sieve_home: root.clone(),
                policy_path: PathBuf::from(DEFAULT_POLICY_PATH),
                event_log_path: event_log_path.clone(),
                runtime_cwd: root.to_string_lossy().to_string(),
                allowed_tools,
                audio_stt_cmd: None,
                audio_tts_cmd: None,
                image_ocr_cmd: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
                max_concurrent_turns: 1,
                max_planner_steps: 3,
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
            let (telegram_event_tx, telegram_event_rx) = mpsc::channel();
            let event_log = Arc::new(
                FanoutRuntimeEventLog::new(event_log_path, telegram_event_tx)
                    .expect("create e2e fanout event log"),
            );
            let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
                shell: Arc::new(BasicShellAnalyzer),
                summaries: Arc::new(DefaultCommandSummarizer),
                policy: Arc::new(policy),
                quarantine: Arc::new(BwrapQuarantineRunner::default()),
                mainline: Arc::new(AppMainlineRunner::new(cfg.sieve_home.join("artifacts"))),
                planner,
                approval_bus: Arc::new(InProcessApprovalBus::new()),
                event_log: event_log.clone(),
                clock: Arc::new(RuntimeClock),
            }));

            Self {
                runtime,
                guidance_model,
                response_model,
                summary_model,
                event_log,
                cfg,
                next_run_index: AtomicU64::new(1),
                root,
                _telegram_event_rx: telegram_event_rx,
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

        async fn run_text_turn(&self, prompt: &str) -> Result<(), String> {
            let run_index = self.next_run_index.fetch_add(1, Ordering::Relaxed);
            run_turn(
                &self.runtime,
                self.guidance_model.as_ref(),
                self.response_model.as_ref(),
                self.summary_model.as_ref(),
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
    async fn e2e_fake_greeting_fast_path_skips_planner_tool_loop() {
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
            0,
            "greeting fast-path should skip planner loop"
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
    fn compose_quality_retry_treats_verbose_pass_as_pass() {
        let composed = "Here is a direct answer.";
        let gate = Some("Quality gate verdict: PASS because the answer is direct.");
        assert!(compose_quality_requires_retry(composed, gate).is_none());
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
        assert_eq!(
            signal,
            Some(PlannerGuidanceSignal::ContinueFetchAdditionalSource)
        );

        let style_signal = compose_quality_followup_signal(
            Some("REVISE: third-person meta narration."),
            &with_refs,
        );
        assert!(style_signal.is_none());
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
        );
        assert!(enforced.contains("https://example.com/a"));
        assert!(enforced.contains("https://example.com/b"));
        assert!(enforced.contains("provided link"));
    }

    #[test]
    fn enforce_link_policy_strips_link_claim_without_available_urls() {
        let message = "Top result is ready. Visit the provided link for details.".to_string();
        let enforced = enforce_link_policy(message, &[]);
        assert!(!enforced.to_ascii_lowercase().contains("provided link"));
    }

    #[test]
    fn chat_only_prompt_detection_covers_basic_small_talk() {
        assert!(is_chat_only_prompt("Hi how are you?"));
        assert!(is_chat_only_prompt("Can you hear me"));
        assert!(is_chat_only_prompt("Hi can you hear me? How are you?"));
        assert!(is_chat_only_prompt("what's your name"));
        assert!(!is_chat_only_prompt(
            "What is the weather in Livermore ca going to be like tomorrow"
        ));
        assert!(!is_chat_only_prompt(
            "What are the top 3 Mexican food restaurants in Livermore ca?"
        ));
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
