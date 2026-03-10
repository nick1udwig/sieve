use super::*;

#[test]
fn fanout_runtime_event_log_allocates_session_scoped_turn_ids() {
    let (tx_a, _rx_a) = mpsc::channel();
    let path_a = std::env::temp_dir().join(format!(
        "sieve-app-event-log-a-{}.jsonl",
        std::process::id()
    ));
    let _ = fs::remove_file(&path_a);
    let log_a = FanoutRuntimeEventLog::with_session_id(path_a.clone(), tx_a, "sess-a".to_string())
        .expect("create fanout log a");
    let turn_a = log_a.reserve_turn("stdin");
    assert_eq!(turn_a.run_id.0, "sess-a-t1");
    assert_eq!(turn_a.turn_seq, 1);

    let (tx_b, _rx_b) = mpsc::channel();
    let path_b = std::env::temp_dir().join(format!(
        "sieve-app-event-log-b-{}.jsonl",
        std::process::id()
    ));
    let _ = fs::remove_file(&path_b);
    let log_b = FanoutRuntimeEventLog::with_session_id(path_b.clone(), tx_b, "sess-b".to_string())
        .expect("create fanout log b");
    let turn_b = log_b.reserve_turn("stdin");
    assert_eq!(turn_b.run_id.0, "sess-b-t1");
    assert_eq!(turn_b.turn_seq, 1);

    let _ = fs::remove_file(path_a);
    let _ = fs::remove_file(path_b);
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
            prompt_kind: sieve_types::ApprovalPromptKind::Command,
            title: None,
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
            preview: None,
            reply_to_session_id: None,
            allow_approve_always: true,
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
    assert_eq!(prompt.session_key, "main");
    assert_eq!(prompt.turn_kind, TurnKind::User);
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
    let log = FanoutRuntimeEventLog::with_session_id(path.clone(), tx, "sess-log".to_string())
        .expect("create fanout log");
    let turn = log.reserve_turn_with_metadata("telegram", "main", "user");
    let event = RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
        schema_version: 1,
        run_id: turn.run_id.clone(),
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
        turn.run_id.clone(),
        ConversationRole::User,
        "hello".to_string(),
        4,
    ))
    .await
    .expect("append conversation");
    let records = read_jsonl_records(&path);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema_version"], Value::from(2));
    assert_eq!(records[0]["session_id"], Value::from("sess-log"));
    assert_eq!(records[0]["turn_id"], Value::from(turn.run_id.0.clone()));
    assert_eq!(records[0]["turn_seq"], Value::from(1));
    assert_eq!(records[0]["source"], Value::from("telegram"));
    assert_eq!(records[0]["logical_session_key"], Value::from("main"));
    assert_eq!(records[0]["turn_kind"], Value::from("user"));
    assert_eq!(records[0]["component"], Value::from("policy"));
    assert_eq!(
        records[0]["payload"]["decision"]["kind"],
        Value::from("allow")
    );
    assert_eq!(records[1]["event"], Value::from("conversation"));
    assert_eq!(records[1]["component"], Value::from("conversation"));
    assert_eq!(records[1]["payload"]["role"], Value::from("user"));
    assert_eq!(records[1]["payload"]["message"], Value::from("hello"));
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
