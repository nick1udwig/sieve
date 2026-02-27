#![forbid(unsafe_code)]

use async_trait::async_trait;
use serde::Serialize;
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge, TelegramPrompt,
};
use sieve_llm::OpenAiPlannerModel;
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{
    ApprovalBusError, BashMainlineRunner, EventLogError, InProcessApprovalBus,
    JsonlRuntimeEventLog, PlannerRunRequest, RuntimeDeps, RuntimeEventLog, RuntimeOrchestrator,
    SystemClock as RuntimeClock,
};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{ApprovalResolvedEvent, RunId, RuntimeEvent, UncertainMode, UnknownMode};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc as tokio_mpsc, Semaphore};

#[derive(Clone)]
struct AppConfig {
    telegram_bot_token: String,
    telegram_chat_id: i64,
    telegram_poll_timeout_secs: u16,
    policy_path: PathBuf,
    event_log_path: PathBuf,
    runtime_cwd: String,
    allowed_tools: Vec<String>,
    unknown_mode: UnknownMode,
    uncertain_mode: UncertainMode,
    max_concurrent_turns: usize,
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

        Ok(Self {
            telegram_bot_token,
            telegram_chat_id,
            telegram_poll_timeout_secs,
            policy_path,
            event_log_path,
            runtime_cwd,
            allowed_tools,
            unknown_mode: parse_unknown_mode(env::var("SIEVE_UNKNOWN_MODE").ok())?,
            uncertain_mode: parse_uncertain_mode(env::var("SIEVE_UNCERTAIN_MODE").ok())?,
            max_concurrent_turns,
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
        if text.is_empty() {
            return;
        }
        if let Some(prompt_tx) = &self.prompt_tx {
            if let Err(err) = prompt_tx.send(IngressPrompt {
                source: PromptSource::Telegram,
                text,
            }) {
                eprintln!("failed to enqueue telegram prompt: {err}");
            }
        }
    }
}

fn format_approval_bus_error(err: &ApprovalBusError) -> String {
    err.to_string()
}

struct FanoutRuntimeEventLog {
    jsonl: JsonlRuntimeEventLog,
    history: Mutex<Vec<RuntimeEvent>>,
    telegram_tx: Mutex<Sender<RuntimeEvent>>,
}

impl FanoutRuntimeEventLog {
    fn new(
        path: impl Into<PathBuf>,
        telegram_tx: Sender<RuntimeEvent>,
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
            .send(event)
            .map_err(|err| {
                EventLogError::Append(format!("failed to forward runtime event: {err}"))
            })?;
        Ok(())
    }
}

fn spawn_telegram_loop(
    cfg: &AppConfig,
    bridge: RuntimeBridge,
    event_rx: Receiver<RuntimeEvent>,
) -> thread::JoinHandle<()> {
    let bot_token = cfg.telegram_bot_token.clone();
    let chat_id = cfg.telegram_chat_id;
    let poll_timeout_secs = cfg.telegram_poll_timeout_secs;

    thread::spawn(move || {
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id,
                poll_timeout_secs,
            },
            bridge,
            TelegramBotApiLongPoll::new(bot_token),
            TelegramClock,
        );

        loop {
            let mut disconnected = false;
            loop {
                match event_rx.try_recv() {
                    Ok(event) => {
                        if let Err(err) = adapter.publish_runtime_event(event) {
                            eprintln!("telegram publish runtime event failed: {err:?}");
                        }
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

async fn run_turn(
    runtime: &RuntimeOrchestrator,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_index: u64,
    user_message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_id = RunId(format!("run-{run_index}"));
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::User,
            user_message.clone(),
            now_ms(),
        ))
        .await?;

    let result = match runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: run_id.clone(),
            cwd: cfg.runtime_cwd.clone(),
            user_message,
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

    println!("{} -> {:?}", run_id.0, result.tool_results);
    if let Some(thoughts) = result.thoughts.as_ref() {
        println!("{} thoughts: {}", run_id.0, thoughts);
    }
    let assistant_message = format!(
        "{} -> {:?}{}",
        run_id.0,
        result.tool_results,
        result
            .thoughts
            .as_ref()
            .map(|thoughts| format!("; thoughts: {thoughts}"))
            .unwrap_or_default()
    );
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id,
            ConversationRole::Assistant,
            assistant_message,
            now_ms(),
        ))
        .await?;
    Ok(())
}

async fn run_agent_loop(
    runtime: Arc<RuntimeOrchestrator>,
    event_log: Arc<FanoutRuntimeEventLog>,
    cfg: AppConfig,
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
        let event_log = event_log.clone();
        let cfg = cfg.clone();
        let source = prompt.source;
        let text = prompt.text;
        let run_index = next_run_id.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) = run_turn(&runtime, &event_log, &cfg, run_index, text).await {
                eprintln!("run-{run_index} ({}) failed: {err}", source.as_str());
            }
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
    let event_log = Arc::new(FanoutRuntimeEventLog::new(
        cfg.event_log_path.clone(),
        event_tx,
    )?);

    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
        quarantine: Arc::new(BwrapQuarantineRunner::default()),
        mainline: Arc::new(BashMainlineRunner),
        planner: Arc::new(planner),
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    }));

    if single_command_mode {
        run_turn(&runtime, &event_log, &cfg, 1, cli_prompt).await?;
        drop(runtime);
        drop(event_log);
        let _ = telegram_thread.join();
    } else {
        run_agent_loop(
            runtime.clone(),
            event_log.clone(),
            cfg.clone(),
            prompt_rx.expect("agent mode prompt receiver missing"),
        )
        .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        });

        let prompt = rx.recv().await.expect("expected prompt");
        assert_eq!(prompt.source, PromptSource::Telegram);
        assert_eq!(prompt.text, "check logs");
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
            event
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
