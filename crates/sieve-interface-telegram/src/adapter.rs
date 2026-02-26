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
    use sieve_types::{
        Action, ApprovalRequestId, Capability, CommandSegment, PolicyDecision, PolicyDecisionKind,
        PolicyEvaluatedEvent, QuarantineCompletedEvent, QuarantineReport, Resource, RunId,
        UnixMillis,
    };
    use std::collections::VecDeque;
    use std::sync::Mutex;

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
}
