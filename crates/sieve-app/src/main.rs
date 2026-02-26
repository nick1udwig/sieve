#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge,
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
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

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
}

impl AppConfig {
    fn from_env() -> Result<Self, String> {
        let telegram_bot_token = required_env("TELEGRAM_BOT_TOKEN")?;
        let telegram_chat_id = required_env("TELEGRAM_CHAT_ID")?
            .parse::<i64>()
            .map_err(|err| format!("invalid TELEGRAM_CHAT_ID: {err}"))?;
        let telegram_poll_timeout_secs = parse_u16_env("SIEVE_TELEGRAM_POLL_TIMEOUT_SECS", 1)?;
        let policy_path = PathBuf::from(required_env("SIEVE_POLICY_PATH")?);
        let event_log_path = env::var("SIEVE_RUNTIME_EVENT_LOG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(".sieve/logs/runtime-events.jsonl"));
        let runtime_cwd = env::var("SIEVE_RUNTIME_CWD").unwrap_or_else(|_| ".".to_string());
        let allowed_tools = parse_allowed_tools(
            &env::var("SIEVE_ALLOWED_TOOLS")
                .unwrap_or_else(|_| "bash,endorse,declassify".to_string()),
        );
        if allowed_tools.is_empty() {
            return Err("SIEVE_ALLOWED_TOOLS must include at least one tool".to_string());
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
        })
    }
}

fn required_env(key: &str) -> Result<String, String> {
    env::var(key).map_err(|_| format!("missing required environment variable `{key}`"))
}

fn parse_u16_env(key: &str, default: u16) -> Result<u16, String> {
    match env::var(key) {
        Ok(raw) => raw
            .parse::<u16>()
            .map_err(|err| format!("invalid {key}: {err}")),
        Err(_) => Ok(default),
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

struct RuntimeBridge {
    approval_bus: Arc<InProcessApprovalBus>,
}

impl RuntimeBridge {
    fn new(approval_bus: Arc<InProcessApprovalBus>) -> Self {
        Self { approval_bus }
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
    approval_bus: Arc<InProcessApprovalBus>,
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
            RuntimeBridge::new(approval_bus),
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

async fn run_turn(
    runtime: &RuntimeOrchestrator,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_index: u64,
    user_message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_id = RunId(format!("run-{run_index}"));
    let result = runtime
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
        .await?;

    println!("{} -> {:?}", run_id.0, result.tool_results);
    if let Some(thoughts) = result.thoughts {
        println!("{} thoughts: {}", run_id.0, thoughts);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg =
        AppConfig::from_env().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let policy_toml = fs::read_to_string(&cfg.policy_path)?;
    let policy = TomlPolicyEngine::from_toml_str(&policy_toml)?;

    let planner = OpenAiPlannerModel::from_env()?;
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let (event_tx, event_rx) = mpsc::channel();
    let telegram_thread = spawn_telegram_loop(&cfg, approval_bus.clone(), event_rx);
    let event_log = Arc::new(FanoutRuntimeEventLog::new(
        cfg.event_log_path.clone(),
        event_tx,
    )?);

    let runtime = RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
        quarantine: Arc::new(BwrapQuarantineRunner::default()),
        mainline: Arc::new(BashMainlineRunner),
        planner: Arc::new(planner),
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    });

    let cli_prompt = env::args().skip(1).collect::<Vec<String>>().join(" ");
    if !cli_prompt.trim().is_empty() {
        run_turn(&runtime, &event_log, &cfg, 1, cli_prompt).await?;
    } else {
        eprintln!("sieve-app ready; enter one prompt per line");
        let stdin = io::stdin();
        let mut run_index = 1_u64;
        for line in stdin.lock().lines() {
            let line = line?;
            let prompt = line.trim();
            if prompt.is_empty() {
                continue;
            }
            run_turn(&runtime, &event_log, &cfg, run_index, prompt.to_string()).await?;
            run_index += 1;
        }
    }

    drop(runtime);
    drop(event_log);
    let _ = telegram_thread.join();
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
        let _ = fs::remove_file(path);
    }
}
