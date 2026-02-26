use crate::{
    message::{
        format_approval_requested, format_policy_evaluated, format_quarantine_completed,
        parse_command, TelegramApprovalAction,
    },
    Clock, TelegramAdapterConfig, TelegramAdapterError, TelegramEventBridge, TelegramLongPoll,
    TelegramUpdate,
};
use sieve_types::{ApprovalAction, ApprovalRequestedEvent, ApprovalResolvedEvent, RuntimeEvent};
use std::collections::BTreeMap;

pub struct TelegramAdapter<B, P, C>
where
    B: TelegramEventBridge,
    P: TelegramLongPoll,
    C: Clock,
{
    config: TelegramAdapterConfig,
    bridge: B,
    poll: P,
    clock: C,
    next_update_offset: Option<i64>,
    pending_approvals: BTreeMap<String, ApprovalRequestedEvent>,
}

impl<B, P, C> TelegramAdapter<B, P, C>
where
    B: TelegramEventBridge,
    P: TelegramLongPoll,
    C: Clock,
{
    pub fn new(config: TelegramAdapterConfig, bridge: B, poll: P, clock: C) -> Self {
        Self {
            config,
            bridge,
            poll,
            clock,
            next_update_offset: None,
            pending_approvals: BTreeMap::new(),
        }
    }

    pub fn publish_runtime_event(
        &mut self,
        event: RuntimeEvent,
    ) -> Result<(), TelegramAdapterError> {
        self.bridge.publish_runtime_event(&event);

        match event {
            RuntimeEvent::ApprovalRequested(event) => {
                let key = event.request_id.0.clone();
                let text = format_approval_requested(&event);
                self.pending_approvals.insert(key, event);
                self.send_to_chat(&text)?;
            }
            RuntimeEvent::PolicyEvaluated(event) => {
                self.send_to_chat(&format_policy_evaluated(&event))?;
            }
            RuntimeEvent::QuarantineCompleted(event) => {
                self.send_to_chat(&format_quarantine_completed(&event))?;
            }
            RuntimeEvent::ApprovalResolved(event) => {
                self.pending_approvals.remove(&event.request_id.0);
            }
        }

        Ok(())
    }

    pub fn poll_once(&mut self) -> Result<(), TelegramAdapterError> {
        let updates = self
            .poll
            .get_updates(self.next_update_offset, self.config.poll_timeout_secs)
            .map_err(TelegramAdapterError::Transport)?;

        for update in updates {
            self.next_update_offset = Some(update.update_id + 1);
            self.handle_update(update)?;
        }

        Ok(())
    }

    pub fn run_long_poll_loop(&mut self) -> Result<(), TelegramAdapterError> {
        loop {
            self.poll_once()?;
        }
    }

    fn handle_update(&mut self, update: TelegramUpdate) -> Result<(), TelegramAdapterError> {
        let Some(message) = update.message else {
            return Ok(());
        };
        if message.chat_id != self.config.chat_id {
            return Ok(());
        }

        let Some(command) = parse_command(&message.text) else {
            return Ok(());
        };

        let Some(approval_requested) = self.pending_approvals.remove(&command.request_id) else {
            self.send_to_chat(&format!("request not found: {}", command.request_id))?;
            return Ok(());
        };

        let action = match command.action {
            TelegramApprovalAction::ApproveOnce => ApprovalAction::ApproveOnce,
            TelegramApprovalAction::Deny => ApprovalAction::Deny,
        };

        let resolved = ApprovalResolvedEvent {
            schema_version: approval_requested.schema_version,
            request_id: approval_requested.request_id,
            run_id: approval_requested.run_id,
            action,
            created_at_ms: self.clock.now_ms(),
        };
        self.bridge.submit_approval(resolved.clone());

        let action_text = match resolved.action {
            ApprovalAction::ApproveOnce => "approve_once",
            ApprovalAction::Deny => "deny",
        };
        self.send_to_chat(&format!(
            "approval resolved: {} {}",
            resolved.request_id.0, action_text
        ))?;

        Ok(())
    }

    fn send_to_chat(&mut self, text: &str) -> Result<(), TelegramAdapterError> {
        self.poll
            .send_message(self.config.chat_id, text)
            .map_err(TelegramAdapterError::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TelegramMessage, TelegramUpdate};
    use async_trait::async_trait;
    use sieve_command_summaries::DefaultCommandSummarizer;
    use sieve_llm::{LlmError, PlannerModel};
    use sieve_policy::TomlPolicyEngine;
    use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
    use sieve_runtime::{
        EventLogError, InProcessApprovalBus, MainlineRunError, MainlineRunReport,
        MainlineRunRequest, MainlineRunner, PlannerRunRequest, RuntimeDeps, RuntimeDisposition,
        RuntimeError, RuntimeEventLog, RuntimeOrchestrator, ShellRunRequest,
        SystemClock as RuntimeSystemClock,
    };
    use sieve_shell::BasicShellAnalyzer;
    use sieve_types::{
        Action, ApprovalRequestId, Capability, CommandSegment, LlmModelConfig, LlmProvider,
        PlannerToolCall, PlannerTurnInput, PlannerTurnOutput, PolicyDecision, PolicyDecisionKind,
        PolicyEvaluatedEvent, QuarantineCompletedEvent, QuarantineReport, QuarantineRunRequest,
        Resource, RunId, UncertainMode, UnixMillis, UnknownMode,
    };
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    struct TestBridge {
        runtime_events: Mutex<Vec<RuntimeEvent>>,
        approvals: Mutex<Vec<ApprovalResolvedEvent>>,
    }

    impl TestBridge {
        fn new() -> Self {
            Self {
                runtime_events: Mutex::new(Vec::new()),
                approvals: Mutex::new(Vec::new()),
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
    }

    struct TestPoller {
        updates: VecDeque<Vec<TelegramUpdate>>,
        sent_messages: Vec<(i64, String)>,
    }

    impl TestPoller {
        fn new(updates: Vec<Vec<TelegramUpdate>>) -> Self {
            Self {
                updates: updates.into(),
                sent_messages: Vec::new(),
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

        fn send_message(&mut self, chat_id: i64, text: &str) -> Result<(), String> {
            self.sent_messages.push((chat_id, text.to_string()));
            Ok(())
        }
    }

    struct FixedClock {
        now: UnixMillis,
    }

    impl Clock for FixedClock {
        fn now_ms(&self) -> UnixMillis {
            self.now
        }
    }

    fn sample_approval_requested() -> ApprovalRequestedEvent {
        ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("apr_1".into()),
            run_id: RunId("run_1".into()),
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

    #[test]
    fn poll_once_maps_approve_once_to_approval_resolved_event() {
        let bridge = TestBridge::new();
        let poller = TestPoller::new(vec![vec![TelegramUpdate {
            update_id: 9,
            message: Some(TelegramMessage {
                chat_id: 42,
                text: "/approve_once apr_1".into(),
            }),
        }]]);
        let clock = FixedClock { now: 777 };
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            bridge,
            poller,
            clock,
        );

        adapter
            .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
            .expect("publish runtime event");
        adapter.poll_once().expect("poll once");

        let approvals = adapter
            .bridge
            .approvals
            .lock()
            .expect("approvals mutex poisoned")
            .clone();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].request_id.0, "apr_1");
        assert_eq!(approvals[0].run_id.0, "run_1");
        assert_eq!(approvals[0].action, ApprovalAction::ApproveOnce);
        assert_eq!(approvals[0].created_at_ms, 777);
    }

    #[test]
    fn poll_once_maps_deny_to_approval_resolved_event() {
        let bridge = TestBridge::new();
        let poller = TestPoller::new(vec![vec![TelegramUpdate {
            update_id: 11,
            message: Some(TelegramMessage {
                chat_id: 42,
                text: "/deny apr_1".into(),
            }),
        }]]);
        let clock = FixedClock { now: 888 };
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            bridge,
            poller,
            clock,
        );

        adapter
            .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
            .expect("publish runtime event");
        adapter.poll_once().expect("poll once");

        let approvals = adapter
            .bridge
            .approvals
            .lock()
            .expect("approvals mutex poisoned")
            .clone();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].action, ApprovalAction::Deny);
    }

    #[test]
    fn formats_approval_policy_and_quarantine_messages() {
        let bridge = TestBridge::new();
        let poller = TestPoller::new(Vec::new());
        let clock = FixedClock { now: 0 };
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            bridge,
            poller,
            clock,
        );

        adapter
            .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
            .expect("approval requested event");
        adapter
            .publish_runtime_event(RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
                schema_version: 1,
                run_id: RunId("run_1".into()),
                decision: PolicyDecision {
                    kind: PolicyDecisionKind::DenyWithApproval,
                    reason: "blocked by policy".into(),
                    blocked_rule_id: Some("deny-rm-rf".into()),
                },
                inferred_capabilities: Vec::new(),
                trace_path: None,
                created_at_ms: 111,
            }))
            .expect("policy evaluated event");
        adapter
            .publish_runtime_event(RuntimeEvent::QuarantineCompleted(
                QuarantineCompletedEvent {
                    schema_version: 1,
                    run_id: RunId("run_1".into()),
                    report: QuarantineReport {
                        run_id: RunId("run_1".into()),
                        trace_path: "/tmp/trace".into(),
                        stdout_path: None,
                        stderr_path: None,
                        attempted_capabilities: Vec::new(),
                        exit_code: Some(1),
                    },
                    created_at_ms: 112,
                },
            ))
            .expect("quarantine completed event");

        let sent_messages = &adapter.poll.sent_messages;
        assert_eq!(sent_messages.len(), 3);
        assert!(sent_messages[0].1.contains("argv: rm -rf /tmp/scratch"));
        assert!(sent_messages[0].1.contains("blocked_rule_id: deny-rm-rf"));
        assert!(sent_messages[0].1.contains("reason: mutating command"));
        assert!(sent_messages[1].1.contains("decision: deny_with_approval"));
        assert!(sent_messages[2].1.contains("trace_path: /tmp/trace"));
    }

    #[test]
    fn approve_alias_is_mapped_to_approve_once() {
        let bridge = TestBridge::new();
        let poller = TestPoller::new(vec![vec![TelegramUpdate {
            update_id: 3,
            message: Some(TelegramMessage {
                chat_id: 42,
                text: "approve apr_1".into(),
            }),
        }]]);
        let clock = FixedClock { now: 1010 };
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            bridge,
            poller,
            clock,
        );

        adapter
            .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
            .expect("publish runtime event");
        adapter.poll_once().expect("poll once");

        let approvals = adapter
            .bridge
            .approvals
            .lock()
            .expect("approvals mutex poisoned")
            .clone();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].action, ApprovalAction::ApproveOnce);
    }

    #[test]
    fn ignores_messages_from_unconfigured_chat() {
        let bridge = TestBridge::new();
        let poller = TestPoller::new(vec![vec![TelegramUpdate {
            update_id: 4,
            message: Some(TelegramMessage {
                chat_id: 7,
                text: "/deny apr_1".into(),
            }),
        }]]);
        let clock = FixedClock { now: 2020 };
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            bridge,
            poller,
            clock,
        );

        adapter
            .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
            .expect("publish runtime event");
        adapter.poll_once().expect("poll once");

        let approvals = adapter
            .bridge
            .approvals
            .lock()
            .expect("approvals mutex poisoned")
            .clone();
        assert!(approvals.is_empty());
    }

    #[test]
    fn unknown_request_id_reports_error_message() {
        let bridge = TestBridge::new();
        let poller = TestPoller::new(vec![vec![TelegramUpdate {
            update_id: 5,
            message: Some(TelegramMessage {
                chat_id: 42,
                text: "/deny apr_missing".into(),
            }),
        }]]);
        let clock = FixedClock { now: 3030 };
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            bridge,
            poller,
            clock,
        );

        adapter
            .publish_runtime_event(RuntimeEvent::ApprovalRequested(sample_approval_requested()))
            .expect("publish runtime event");
        adapter.poll_once().expect("poll once");

        let sent_messages = &adapter.poll.sent_messages;
        let last = sent_messages.last().expect("expected at least one message");
        assert!(last.1.contains("request not found: apr_missing"));

        let approvals = adapter
            .bridge
            .approvals
            .lock()
            .expect("approvals mutex poisoned")
            .clone();
        assert!(approvals.is_empty());
    }

    #[derive(Clone, Default)]
    struct SharedPoller {
        updates: Arc<Mutex<VecDeque<Vec<TelegramUpdate>>>>,
        sent_messages: Arc<Mutex<Vec<(i64, String)>>>,
    }

    impl SharedPoller {
        fn push_updates(&self, updates: Vec<TelegramUpdate>) {
            self.updates
                .lock()
                .expect("shared updates mutex poisoned")
                .push_back(updates);
        }

        fn sent_messages(&self) -> Vec<(i64, String)> {
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

        fn send_message(&mut self, chat_id: i64, text: &str) -> Result<(), String> {
            self.sent_messages
                .lock()
                .expect("shared sent messages mutex poisoned")
                .push((chat_id, text.to_string()));
            Ok(())
        }
    }

    struct RuntimeBridge {
        approval_bus: Arc<InProcessApprovalBus>,
        runtime_events: Mutex<Vec<RuntimeEvent>>,
        submit_errors: Mutex<Vec<String>>,
    }

    impl RuntimeBridge {
        fn new(approval_bus: Arc<InProcessApprovalBus>) -> Self {
            Self {
                approval_bus,
                runtime_events: Mutex::new(Vec::new()),
                submit_errors: Mutex::new(Vec::new()),
            }
        }

        fn runtime_events(&self) -> Vec<RuntimeEvent> {
            self.runtime_events
                .lock()
                .expect("runtime bridge events mutex poisoned")
                .clone()
        }

        fn submit_errors(&self) -> Vec<String> {
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
    struct CapturingRuntimeEventLog {
        events: Mutex<Vec<RuntimeEvent>>,
    }

    impl CapturingRuntimeEventLog {
        fn snapshot(&self) -> Vec<RuntimeEvent> {
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

    struct NoopQuarantineRunner;

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

    struct NoopMainlineRunner;

    #[async_trait]
    impl MainlineRunner for NoopMainlineRunner {
        async fn run(
            &self,
            request: MainlineRunRequest,
        ) -> Result<MainlineRunReport, MainlineRunError> {
            Ok(MainlineRunReport {
                run_id: request.run_id,
                exit_code: Some(0),
            })
        }
    }

    struct StaticPlanner {
        config: LlmModelConfig,
        output: PlannerTurnOutput,
    }

    impl StaticPlanner {
        fn new(output: PlannerTurnOutput) -> Self {
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

    fn mk_runtime(
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

    #[tokio::test]
    async fn runtime_approval_roundtrip_works_with_telegram_adapter() {
        let (runtime, approval_bus, event_log) = mk_runtime(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        });
        let poller = SharedPoller::default();
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            RuntimeBridge::new(approval_bus.clone()),
            poller.clone(),
            FixedClock { now: 4444 },
        );

        let runtime_task = tokio::spawn({
            let runtime = runtime.clone();
            async move {
                runtime
                    .orchestrate_shell(ShellRunRequest {
                        run_id: RunId("run_runtime_telegram".to_string()),
                        cwd: "/tmp".to_string(),
                        script: "rm -rf /tmp/scratch".to_string(),
                        control_value_refs: BTreeSet::new(),
                        control_endorsed_by: None,
                        unknown_mode: UnknownMode::Deny,
                        uncertain_mode: UncertainMode::Deny,
                    })
                    .await
            }
        });

        let mut forwarded = 0usize;
        let mut request_id = None;
        for _ in 0..80 {
            let snapshot = event_log.snapshot();
            for event in snapshot.iter().skip(forwarded).cloned() {
                if let RuntimeEvent::ApprovalRequested(requested) = &event {
                    request_id = Some(requested.request_id.0.clone());
                }
                adapter
                    .publish_runtime_event(event)
                    .expect("forward runtime event to telegram");
            }
            forwarded = snapshot.len();
            if request_id.is_some() {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }

        let request_id = request_id.expect("runtime did not emit approval request");
        poller.push_updates(vec![TelegramUpdate {
            update_id: 1,
            message: Some(TelegramMessage {
                chat_id: 42,
                text: format!("/approve_once {request_id}"),
            }),
        }]);
        adapter.poll_once().expect("telegram poll once");

        let disposition = timeout(Duration::from_secs(2), runtime_task)
            .await
            .expect("runtime task timed out")
            .expect("runtime task join failed")
            .expect("runtime orchestration failed");
        match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                assert_eq!(report.run_id, RunId("run_runtime_telegram".to_string()));
                assert_eq!(report.exit_code, Some(0));
            }
            other => panic!("expected mainline execution, got {other:?}"),
        }

        let final_events = event_log.snapshot();
        for event in final_events.iter().skip(forwarded).cloned() {
            adapter
                .publish_runtime_event(event)
                .expect("forward remaining runtime event");
        }

        assert!(final_events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ApprovalResolved(_))));
        assert!(adapter.bridge.submit_errors().is_empty());

        let sent_messages = poller.sent_messages();
        assert!(sent_messages
            .iter()
            .any(|(_, text)| text.contains("approval requested")));
        assert!(sent_messages.iter().any(|(_, text)| {
            text.contains(&format!("approval resolved: {request_id} approve_once"))
        }));
        assert!(adapter
            .bridge
            .runtime_events()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ApprovalRequested(_))));
    }

    #[tokio::test]
    async fn tool_contract_failure_stays_internal_not_chat_visible() {
        let mut args = BTreeMap::new();
        args.insert(
            "cmd".to_string(),
            serde_json::json!(["rm", "-rf", "/tmp/scratch"]),
        );
        let planner_output = PlannerTurnOutput {
            thoughts: Some("invalid args shape".to_string()),
            tool_calls: vec![PlannerToolCall {
                tool_name: "bash".to_string(),
                args,
            }],
        };
        let (runtime, approval_bus, event_log) = mk_runtime(planner_output);
        let poller = SharedPoller::default();
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id: 42,
                poll_timeout_secs: 1,
            },
            RuntimeBridge::new(approval_bus),
            poller.clone(),
            FixedClock { now: 5555 },
        );

        let err = runtime
            .orchestrate_planner_turn(PlannerRunRequest {
                run_id: RunId("run_contract_failure".to_string()),
                cwd: "/tmp".to_string(),
                user_message: "dangerous".to_string(),
                allowed_tools: vec!["bash".to_string()],
                previous_events: Vec::new(),
                control_value_refs: BTreeSet::new(),
                control_endorsed_by: None,
                unknown_mode: UnknownMode::Deny,
                uncertain_mode: UncertainMode::Deny,
            })
            .await
            .expect_err("planner contract validation must fail");

        match err {
            RuntimeError::ToolContract { report } => {
                assert!(!report.errors.is_empty());
            }
            other => panic!("expected tool contract runtime error, got {other:?}"),
        }

        let runtime_events = event_log.snapshot();
        assert!(runtime_events.is_empty());
        for event in runtime_events {
            adapter
                .publish_runtime_event(event)
                .expect("forward runtime event");
        }
        assert!(poller.sent_messages().is_empty());
        assert!(adapter.bridge.runtime_events().is_empty());
    }
}
