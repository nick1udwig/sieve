use super::*;
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

pub(crate) struct TelegramFlowResult {
    pub(crate) sent_messages: Vec<(i64, String)>,
    pub(crate) sent_chat_actions: Vec<(i64, String)>,
}

pub(crate) struct AppE2eHarness {
    runtime: Arc<RuntimeOrchestrator>,
    _automation: Option<Arc<AutomationManager>>,
    approval_bus: Arc<InProcessApprovalBus>,
    guidance_model: Arc<dyn GuidanceModel>,
    response_model: Arc<dyn ResponseModel>,
    summary_model: Arc<dyn SummaryModel>,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: Arc<FanoutRuntimeEventLog>,
    telegram_event_tx: Sender<TelegramLoopEvent>,
    telegram_event_rx: StdMutex<Receiver<TelegramLoopEvent>>,
    pub(crate) cfg: AppConfig,
    pub(crate) root: PathBuf,
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

    pub(crate) fn new(
        model_mode: E2eModelMode,
        allowed_tools: Vec<String>,
        policy_toml: &str,
    ) -> Self {
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
            automation_store_path: root.join("state/automation.json"),
            codex_store_path: root.join("state/codex.db"),
            runtime_cwd: root.to_string_lossy().to_string(),
            heartbeat_interval_ms: None,
            heartbeat_prompt_override: None,
            heartbeat_file_path: root.join("HEARTBEAT.md"),
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
        let automation = if cfg.allowed_tools.iter().any(|tool| tool == "automation") {
            let (prompt_tx, _prompt_rx) = tokio_mpsc::unbounded_channel();
            Some(Arc::new(
                AutomationManager::new(&cfg, prompt_tx, Arc::new(RuntimeClock))
                    .expect("create automation manager"),
            ))
        } else {
            None
        };
        let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
            shell: Arc::new(BasicShellAnalyzer),
            summaries: Arc::new(DefaultCommandSummarizer),
            policy: Arc::new(policy),
            quarantine: Arc::new(BwrapQuarantineRunner::default()),
            mainline: Arc::new(AppMainlineRunner::new(cfg.sieve_home.join("artifacts"))),
            planner,
            automation: automation
                .clone()
                .map(|manager| -> Arc<dyn sieve_runtime::AutomationTool> { manager }),
            codex: None,
            approval_bus: approval_bus.clone(),
            event_log: event_log.clone(),
            clock: Arc::new(RuntimeClock),
        }));

        Self {
            runtime,
            _automation: automation,
            approval_bus,
            guidance_model,
            response_model,
            summary_model,
            lcm: None,
            event_log,
            telegram_event_tx,
            telegram_event_rx: StdMutex::new(telegram_event_rx),
            cfg,
            root,
        }
    }

    pub(crate) fn live_openai_or_skip(allowed_tools: Vec<String>) -> Option<Self> {
        if std::env::var("SIEVE_RUN_OPENAI_LIVE").ok().as_deref() != Some("1") {
            return None;
        }

        Some(Self::new(
            E2eModelMode::RealOpenAi,
            allowed_tools,
            E2E_POLICY_BASE,
        ))
    }

    pub(crate) fn with_lcm(mut self, lcm: Option<Arc<LcmIntegration>>) -> Self {
        self.lcm = lcm;
        self
    }

    pub(crate) async fn run_prompt_turn(
        &self,
        prompt: IngressPrompt,
    ) -> Result<TurnOutcome, String> {
        let reserved_turn = self.event_log.reserve_turn_with_metadata(
            prompt.source.as_str(),
            &prompt.session_key,
            prompt.turn_kind.as_str(),
        );
        run_turn(
            &self.runtime,
            self.guidance_model.as_ref(),
            self.response_model.as_ref(),
            self.summary_model.as_ref(),
            self.lcm.clone(),
            &self.event_log,
            &self.cfg,
            reserved_turn.run_id,
            &prompt,
        )
        .await
        .map(|outcome| outcome)
        .map_err(|err| err.to_string())
    }

    pub(crate) async fn run_text_turn(&self, prompt: &str) -> Result<(), String> {
        self.run_prompt_turn(IngressPrompt::user(
            PromptSource::Stdin,
            prompt.to_string(),
            InteractionModality::Text,
            None,
        ))
        .await
        .map(|_| ())
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

    pub(crate) async fn run_telegram_text_turn(
        &self,
        text: &str,
    ) -> Result<TelegramFlowResult, String> {
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

        let reserved_turn = self.event_log.reserve_turn_with_metadata(
            PromptSource::Telegram.as_str(),
            &ingress.session_key,
            ingress.turn_kind.as_str(),
        );
        let typing_guard = TypingGuard::start(
            self.telegram_event_tx.clone(),
            reserved_turn.run_id.0.clone(),
        )
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
            reserved_turn.run_id,
            &ingress,
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

    pub(crate) fn runtime_events(&self) -> Vec<RuntimeEvent> {
        self.event_log.snapshot()
    }

    pub(crate) fn jsonl_records(&self) -> Vec<Value> {
        read_jsonl_records(&self.cfg.event_log_path)
    }
}

pub(crate) fn read_jsonl_records(path: &Path) -> Vec<Value> {
    let Ok(body) = fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

pub(crate) fn conversation_messages(records: &[Value]) -> Vec<(String, String)> {
    records
        .iter()
        .filter(|record| record.get("event").and_then(Value::as_str) == Some("conversation"))
        .map(|record| {
            let payload = record.get("payload").unwrap_or(record);
            (
                payload
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

pub(crate) fn assistant_messages(events: &[RuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            RuntimeEvent::AssistantMessage(event) => Some(event.message.clone()),
            _ => None,
        })
        .collect()
}

pub(crate) fn count_approval_requested(events: &[RuntimeEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, RuntimeEvent::ApprovalRequested(_)))
        .count()
}

pub(crate) fn assistant_errors_from_conversation(records: &[Value]) -> Vec<String> {
    conversation_messages(records)
        .into_iter()
        .filter(|(role, message)| role == "assistant" && message.starts_with("error:"))
        .map(|(_, message)| message)
        .collect()
}

pub(crate) fn message_contains_plain_url(message: &str) -> bool {
    message.contains("https://") || message.contains("http://")
}

pub(crate) fn latest_telegram_message(flow: &TelegramFlowResult) -> Option<&str> {
    flow.sent_messages
        .last()
        .map(|(_, message)| message.as_str())
}

pub(crate) fn message_has_weather_signal(message: &str) -> bool {
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
