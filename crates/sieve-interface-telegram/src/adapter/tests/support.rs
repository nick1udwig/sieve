use super::*;

pub(super) struct TestBridge {
    pub(super) runtime_events: Mutex<Vec<RuntimeEvent>>,
    pub(super) approvals: Mutex<Vec<ApprovalResolvedEvent>>,
    pub(super) prompts: Mutex<Vec<TelegramPrompt>>,
}

impl TestBridge {
    pub(super) fn new() -> Self {
        Self {
            runtime_events: Mutex::new(Vec::new()),
            approvals: Mutex::new(Vec::new()),
            prompts: Mutex::new(Vec::new()),
        }
    }
}

impl TelegramEventBridge for TestBridge {
    fn publish_runtime_event(&self, event: &RuntimeEvent) {
        self.runtime_events
            .lock()
            .expect("runtime events mutex poisoned")
            .push(event.clone());
    }

    fn submit_approval(&self, approval: ApprovalResolvedEvent) {
        self.approvals
            .lock()
            .expect("approvals mutex poisoned")
            .push(approval);
    }

    fn submit_prompt(&self, prompt: TelegramPrompt) {
        self.prompts
            .lock()
            .expect("prompts mutex poisoned")
            .push(prompt);
    }
}

pub(super) struct TestPoller {
    pub(super) updates: VecDeque<Vec<TelegramUpdate>>,
    pub(super) sent_messages: Vec<(i64, String)>,
    pub(super) sent_chat_actions: Vec<(i64, String)>,
    next_message_id: i64,
}

impl TestPoller {
    pub(super) fn new(updates: Vec<Vec<TelegramUpdate>>) -> Self {
        Self {
            updates: updates.into(),
            sent_messages: Vec::new(),
            sent_chat_actions: Vec::new(),
            next_message_id: 1,
        }
    }
}

impl TelegramLongPoll for TestPoller {
    fn get_updates(
        &mut self,
        _offset: Option<i64>,
        _timeout_secs: u16,
    ) -> Result<Vec<TelegramUpdate>, String> {
        Ok(self.updates.pop_front().unwrap_or_default())
    }

    fn send_message(&mut self, chat_id: i64, text: &str) -> Result<Option<i64>, String> {
        self.sent_messages.push((chat_id, text.to_string()));
        let message_id = self.next_message_id;
        self.next_message_id += 1;
        Ok(Some(message_id))
    }

    fn send_chat_action(&mut self, chat_id: i64, action: &str) -> Result<(), String> {
        self.sent_chat_actions.push((chat_id, action.to_string()));
        Ok(())
    }
}

pub(super) struct FixedClock {
    pub(super) now: UnixMillis,
}

impl Clock for FixedClock {
    fn now_ms(&self) -> UnixMillis {
        self.now
    }
}

pub(super) struct StepClock {
    now: std::sync::atomic::AtomicU64,
    step: u64,
}

impl StepClock {
    pub(super) fn new(start: u64, step: u64) -> Self {
        Self {
            now: std::sync::atomic::AtomicU64::new(start),
            step,
        }
    }
}

impl Clock for StepClock {
    fn now_ms(&self) -> UnixMillis {
        self.now
            .fetch_add(self.step, std::sync::atomic::Ordering::Relaxed)
    }
}

pub(super) fn sample_approval_requested_with_id(
    request_id: &str,
    run_id: &str,
) -> ApprovalRequestedEvent {
    ApprovalRequestedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId(request_id.to_string()),
        run_id: RunId(run_id.to_string()),
        command_segments: vec![CommandSegment {
            argv: vec!["rm".into(), "-rf".into(), "/tmp/scratch".into()],
            operator_before: None,
        }],
        inferred_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp/scratch".into(),
        }],
        blocked_rule_id: "deny-rm-rf".into(),
        reason: "mutating command".into(),
        created_at_ms: 100,
    }
}

pub(super) fn sample_approval_requested() -> ApprovalRequestedEvent {
    sample_approval_requested_with_id("apr_1", "run_1")
}

pub(super) fn test_config(
    allowed_sender_user_ids: Option<BTreeSet<i64>>,
) -> TelegramAdapterConfig {
    TelegramAdapterConfig {
        chat_id: 42,
        poll_timeout_secs: 1,
        allowed_sender_user_ids,
    }
}

#[derive(Clone, Default)]
pub(super) struct SharedPoller {
    updates: Arc<Mutex<VecDeque<Vec<TelegramUpdate>>>>,
    sent_messages: Arc<Mutex<Vec<(i64, String)>>>,
    sent_chat_actions: Arc<Mutex<Vec<(i64, String)>>>,
    next_message_id: Arc<Mutex<i64>>,
}

impl SharedPoller {
    pub(super) fn push_updates(&self, updates: Vec<TelegramUpdate>) {
        self.updates
            .lock()
            .expect("shared updates mutex poisoned")
            .push_back(updates);
    }

    pub(super) fn sent_messages(&self) -> Vec<(i64, String)> {
        self.sent_messages
            .lock()
            .expect("shared sent messages mutex poisoned")
            .clone()
    }
}

impl TelegramLongPoll for SharedPoller {
    fn get_updates(
        &mut self,
        _offset: Option<i64>,
        _timeout_secs: u16,
    ) -> Result<Vec<TelegramUpdate>, String> {
        Ok(self
            .updates
            .lock()
            .expect("shared updates mutex poisoned")
            .pop_front()
            .unwrap_or_default())
    }

    fn send_message(&mut self, chat_id: i64, text: &str) -> Result<Option<i64>, String> {
        self.sent_messages
            .lock()
            .expect("shared sent messages mutex poisoned")
            .push((chat_id, text.to_string()));
        let mut next_id = self
            .next_message_id
            .lock()
            .expect("shared next message id mutex poisoned");
        let message_id = *next_id;
        *next_id += 1;
        Ok(Some(message_id))
    }

    fn send_chat_action(&mut self, chat_id: i64, action: &str) -> Result<(), String> {
        self.sent_chat_actions
            .lock()
            .expect("shared chat actions mutex poisoned")
            .push((chat_id, action.to_string()));
        Ok(())
    }
}

pub(super) struct RuntimeBridge {
    pub(super) approval_bus: Arc<InProcessApprovalBus>,
    runtime_events: Mutex<Vec<RuntimeEvent>>,
    submit_errors: Mutex<Vec<String>>,
}

impl RuntimeBridge {
    pub(super) fn new(approval_bus: Arc<InProcessApprovalBus>) -> Self {
        Self {
            approval_bus,
            runtime_events: Mutex::new(Vec::new()),
            submit_errors: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn runtime_events(&self) -> Vec<RuntimeEvent> {
        self.runtime_events
            .lock()
            .expect("runtime bridge events mutex poisoned")
            .clone()
    }

    pub(super) fn submit_errors(&self) -> Vec<String> {
        self.submit_errors
            .lock()
            .expect("runtime bridge submit errors mutex poisoned")
            .clone()
    }
}

impl TelegramEventBridge for RuntimeBridge {
    fn publish_runtime_event(&self, event: &RuntimeEvent) {
        self.runtime_events
            .lock()
            .expect("runtime bridge events mutex poisoned")
            .push(event.clone());
    }

    fn submit_approval(&self, approval: ApprovalResolvedEvent) {
        if let Err(err) = self.approval_bus.resolve(approval) {
            eprintln!("telegram bridge failed to resolve approval: {err}");
            self.submit_errors
                .lock()
                .expect("runtime bridge submit errors mutex poisoned")
                .push(err.to_string());
        }
    }
}

#[derive(Default)]
pub(super) struct CapturingRuntimeEventLog {
    events: Mutex<Vec<RuntimeEvent>>,
}

impl CapturingRuntimeEventLog {
    pub(super) fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.events
            .lock()
            .expect("runtime event log mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl RuntimeEventLog for CapturingRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        self.events
            .lock()
            .map_err(|_| EventLogError::Append("runtime event log mutex poisoned".into()))?
            .push(event);
        Ok(())
    }
}

pub(super) struct NoopQuarantineRunner;

#[async_trait]
impl QuarantineRunner for NoopQuarantineRunner {
    async fn run(
        &self,
        request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError> {
        Ok(QuarantineReport {
            run_id: request.run_id,
            trace_path: "/tmp/unused-trace".into(),
            stdout_path: None,
            stderr_path: None,
            attempted_capabilities: Vec::new(),
            exit_code: Some(0),
        })
    }
}

pub(super) struct NoopMainlineRunner;

#[async_trait]
impl MainlineRunner for NoopMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: Some(0),
            artifacts: Vec::new(),
        })
    }
}

pub(super) struct StaticPlanner {
    config: LlmModelConfig,
    output: PlannerTurnOutput,
}

impl StaticPlanner {
    pub(super) fn new(output: PlannerTurnOutput) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "test-planner".to_string(),
                api_base: None,
            },
            output,
        }
    }
}

#[async_trait]
impl PlannerModel for StaticPlanner {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, _input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        Ok(self.output.clone())
    }
}

pub(super) fn mk_runtime(
    planner_output: PlannerTurnOutput,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<InProcessApprovalBus>,
    Arc<CapturingRuntimeEventLog>,
) {
    let policy = TomlPolicyEngine::from_toml_str(
        r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"
reason = "rm -rf requires approval"
"#,
    )
    .expect("policy config must parse");
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(CapturingRuntimeEventLog::default());
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
        quarantine: Arc::new(NoopQuarantineRunner),
        mainline: Arc::new(NoopMainlineRunner),
        planner: Arc::new(StaticPlanner::new(planner_output)),
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeSystemClock),
    }));
    (runtime, approval_bus, event_log)
}
