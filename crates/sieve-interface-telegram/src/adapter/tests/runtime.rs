use super::support::{mk_runtime, test_config, FixedClock, RuntimeBridge, SharedPoller};
use super::*;

#[tokio::test]
async fn runtime_approval_roundtrip_works_with_telegram_adapter() {
    let (runtime, approval_bus, event_log) = mk_runtime(PlannerTurnOutput {
        thoughts: None,
        tool_calls: Vec::new(),
    });
    let poller = SharedPoller::default();
    let mut adapter = TelegramAdapter::new(
        test_config(None),
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
                    script: "trash -f /tmp/scratch".to_string(),
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
            sender_user_id: Some(1001),
            message_id: 1_001,
            reply_to_message_id: None,
            text: format!("/approve_once {request_id}"),
        }),
        message_reaction: None,
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
        .any(|(_, text)| text.contains("approval needed")));
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
        test_config(None),
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
            allowed_net_connect_scopes: Vec::new(),
            browser_sessions: Vec::new(),
            previous_events: Vec::new(),
            guidance: None,
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
