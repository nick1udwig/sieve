use super::*;
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
