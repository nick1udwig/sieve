use super::*;

#[tokio::test]
async fn jsonl_event_log_appends_in_order() {
    let path = temp_dir().join(format!("sieve-runtime-events-{}.jsonl", std::process::id()));
    let _ = remove_file(&path);
    let log = JsonlRuntimeEventLog::new(&path).expect("create log");

    log.append(RuntimeEvent::ApprovalRequested(ApprovalRequestedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("approval-1".to_string()),
        run_id: RunId("run-1".to_string()),
        prompt_kind: sieve_types::ApprovalPromptKind::Command,
        title: None,
        command_segments: vec![CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
            operator_before: None,
        }],
        inferred_capabilities: Vec::new(),
        blocked_rule_id: "rule-1".to_string(),
        reason: "needs approval".to_string(),
        preview: None,
        allow_approve_always: true,
        created_at_ms: 1000,
    }))
    .await
    .expect("append request");

    log.append(RuntimeEvent::ApprovalResolved(ApprovalResolvedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("approval-1".to_string()),
        run_id: RunId("run-1".to_string()),
        action: ApprovalAction::ApproveOnce,
        created_at_ms: 1001,
    }))
    .await
    .expect("append resolution");

    let body = read_to_string(&path).expect("read log file");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: RuntimeEvent = serde_json::from_str(lines[0]).expect("parse first event");
    let second: RuntimeEvent = serde_json::from_str(lines[1]).expect("parse second event");
    assert!(matches!(first, RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(second, RuntimeEvent::ApprovalResolved(_)));

    let _ = remove_file(path);
}

#[tokio::test]
async fn jsonl_event_log_appends_custom_json_records() {
    let path = temp_dir().join(format!(
        "sieve-runtime-records-{}.jsonl",
        std::process::id()
    ));
    let _ = remove_file(&path);
    let log = JsonlRuntimeEventLog::new(&path).expect("create log");

    log.append_json_value(&serde_json::json!({
        "event": "conversation",
        "schema_version": 1,
        "run_id": "run-1",
        "role": "user",
        "message": "hello",
        "created_at_ms": 1002
    }))
    .await
    .expect("append custom record");

    let body = read_to_string(&path).expect("read log file");
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 1);
    let record: serde_json::Value = serde_json::from_str(lines[0]).expect("parse json record");
    assert_eq!(record["event"], "conversation");
    assert_eq!(record["role"], "user");
    assert_eq!(record["message"], "hello");

    let _ = remove_file(path);
}
