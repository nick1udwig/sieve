use super::*;
use serde_json::Value;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter as TestTelegramAdapter, TelegramAdapterConfig,
    TelegramEventBridge, TelegramLongPoll, TelegramMessage as TestTelegramMessage,
    TelegramPrompt, TelegramUpdate as TestTelegramUpdate,
};
use sieve_llm::{GuidanceModel, LlmError, PlannerModel};
use sieve_runtime::ApprovalBus;
use sieve_types::{
    ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, CommandSegment, LlmModelConfig,
    LlmProvider, PlannerGuidanceFrame, PlannerGuidanceInput, PlannerGuidanceOutput,
    PlannerGuidanceSignal, PlannerToolCall, PlannerTurnInput, PlannerTurnOutput, PolicyDecision,
    PolicyDecisionKind, PolicyEvaluatedEvent, Resource,
};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
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
            return Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":null}".to_string());
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
            return Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":null}".to_string());
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
                Ok(TelegramLoopEvent::Runtime(event)) => adapter
                    .publish_runtime_event(event)
                    .map_err(|err| format!("telegram publish runtime event failed: {err:?}"))?,
                Ok(TelegramLoopEvent::TypingStart { run_id }) => adapter
                    .start_typing(run_id)
                    .map_err(|err| format!("telegram start typing failed: {err:?}"))?,
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
    let draft = "Thanks for sharing that you live in Livermore, CA. What can I help with today?"
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
    let message =
        latest_telegram_message(&flow).expect("live telegram weather today should send message");
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
    let message =
        latest_telegram_message(&flow).expect("live telegram weather tomorrow should send message");
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
    let args = media::st_audio_stt_args(Path::new("/tmp/input.ogg"));
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
fn st_audio_tts_args_force_opus_format() {
    let args = media::st_audio_tts_args(Path::new("/tmp/tts-input.txt"), Path::new("/tmp/out.ogg"));
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
            "opus".to_string(),
            "--output".to_string(),
            "/tmp/out.ogg".to_string(),
        ]
    );
}

#[test]
fn codex_image_ocr_args_include_read_only_ephemeral_image_prompt() {
    let args = media::codex_image_ocr_args(Path::new("/tmp/photo.png"));
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
            media::CODEX_IMAGE_OCR_PROMPT.to_string(),
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

    let (input, refs) =
        build_response_turn_input(&run_id, "hi", InteractionModality::Text, &planner_result);
    assert_eq!(input.run_id, run_id);
    assert_eq!(input.trusted_user_message, "hi");
    assert_eq!(input.response_modality, InteractionModality::Text);
    assert_eq!(input.planner_thoughts.as_deref(), Some("chat reply"));
    assert!(input.tool_outcomes.is_empty());
    assert!(refs.is_empty());
}

#[test]
fn requires_output_visibility_detects_non_empty_stdout_or_stderr_refs() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "show output".to_string(),
        response_modality: InteractionModality::Text,
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
            response_modality: InteractionModality::Text,
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
        response_modality: InteractionModality::Text,
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
        response_modality: InteractionModality::Text,
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
    assert!(gate_requires_retry(Some("This response lacks specific weather details.")).is_some());
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
        response_modality: InteractionModality::Text,
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
        command: "sieve-lcm-cli query --lane both --query \"where do i live\" --json".to_string(),
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
        response_modality: InteractionModality::Text,
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

    let style_signal =
        compose_quality_followup_signal(Some("REVISE: third-person meta narration."), &with_refs);
    assert!(style_signal.is_none());
}

#[test]
fn compose_quality_followup_maps_required_parameter_signal() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "where do i live".to_string(),
        response_modality: InteractionModality::Text,
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
        response_modality: InteractionModality::Text,
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
        response_modality: InteractionModality::Text,
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
        response_modality: InteractionModality::Text,
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
        response_modality: InteractionModality::Text,
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
            command: "curl -sS \"https://markdown.new/https://example.com/path?x=1\"".to_string(),
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
fn enforce_link_policy_keeps_actionable_link_wording() {
    let message = "Yes—I can read your message here and respond. If you mean actual audio, upload an audio file (or share a link) and tell me what you want (e.g., transcription or a summary).".to_string();
    let enforced = enforce_link_policy(message.clone(), &[], "just answer briefly");
    assert_eq!(enforced, message);
}

#[test]
fn enforce_link_policy_keeps_source_origin_wording() {
    let message = "Strength through Unity, Unity through Faith is best known as a Norsefire slogan in V for Vendetta. If you tell me what you need (e.g., identify the source, quote context, or explain the reference), I can help.".to_string();
    let enforced = enforce_link_policy(message.clone(), &[], "just answer briefly");
    assert_eq!(enforced, message);
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
            "https://www.accuweather.com/en/us/livermore/94550/weather-forecast/337125".to_string()
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
