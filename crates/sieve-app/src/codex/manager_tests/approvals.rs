#[tokio::test]
async fn codex_manager_accepts_live_steer_for_active_session() {
    let root = unique_test_root("codex-steer");
    let script = write_mock_server(
        &root,
        r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id": msg["id"], "result": {"ok": True}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/start":
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_steer"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_steer"}}}), flush=True)
        steer = json.loads(sys.stdin.readline())
        assert steer["method"] == "turn/steer"
        assert steer["params"]["threadId"] == "thr_steer"
        assert steer["params"]["expectedTurnId"] == "turn_steer"
        assert steer["params"]["input"][0]["text"] == "focus on failing tests first"
        print(json.dumps({"id": steer["id"], "result": {"turnId": "turn_steer"}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_steer", "text": "{\"status\":\"completed\",\"summary\":\"steer received\",\"user_visible\":\"steer ok\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_steer", "threadId": "thr_steer", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = Arc::new(test_manager(
        &root,
        &script,
        approval_bus,
        event_log.clone(),
    ));
    let manager_task = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .run_session(CodexSessionRequest {
                    session_id: None,
                    instruction: "inspect the repo".to_string(),
                    sandbox: CodexSandboxMode::WorkspaceWrite,
                    cwd: Some("/tmp/repo".to_string()),
                    writable_roots: vec!["/tmp/repo".to_string()],
                    local_images: Vec::new(),
                })
                .await
        })
    };

    let session = wait_for_active_planner_session(manager.as_ref()).await;
    manager
        .steer_session(&session.session_id, "focus on failing tests first")
        .await
        .expect("steer session");

    let result = manager_task
        .await
        .expect("join manager task")
        .expect("manager result");
    assert_eq!(result.result.user_visible.as_deref(), Some("steer ok"));

    let events = event_log.events.lock().expect("event log lock");
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::CodexSessionStatus(status)
            if status.last_step.contains("received user steer")
    )));
}
#[tokio::test]
async fn codex_manager_file_change_approval_uses_session_prefix_and_preview() {
    let root = unique_test_root("codex-approval");
    let script = write_mock_server(
        &root,
        r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id": msg["id"], "result": {"ok": True}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/start":
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_patch"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_patch"}}}), flush=True)
        print(json.dumps({"method": "item/started", "params": {"item": {"type": "fileChange", "id": "patch_1", "changes": [{"path": "src/main.rs", "diff": "@@ -1 +1 @@\\n-old\\n+new"}]}}}), flush=True)
        print(json.dumps({"id": 91, "method": "item/fileChange/requestApproval", "params": {"threadId": "thr_patch", "turnId": "turn_patch", "itemId": "patch_1", "reason": "apply patch"}}), flush=True)
        response = json.loads(sys.stdin.readline())
        assert response["result"]["decision"] == "acceptForSession"
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_patch", "text": "{\"status\":\"completed\",\"summary\":\"patch applied\",\"user_visible\":\"patch done\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_patch", "threadId": "thr_patch", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = Arc::new(test_manager(
        &root,
        &script,
        approval_bus.clone(),
        event_log.clone(),
    ));
    let manager_task = {
        let manager = manager.clone();
        tokio::spawn(async move {
            manager
                .run_session(CodexSessionRequest {
                    session_id: None,
                    instruction: "apply the patch".to_string(),
                    sandbox: CodexSandboxMode::WorkspaceWrite,
                    cwd: Some("/tmp/repo".to_string()),
                    writable_roots: vec!["/tmp/repo".to_string()],
                    local_images: Vec::new(),
                })
                .await
        })
    };

    let requested = wait_for_published_approval(&approval_bus).await;
    assert_eq!(requested.prompt_kind, ApprovalPromptKind::FileChange);
    assert!(requested
        .title
        .as_deref()
        .is_some_and(|value| value.contains("apply-patch")));
    assert!(requested
        .preview
        .as_deref()
        .is_some_and(|value| value.contains("src/main.rs")));
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveAlways,
            created_at_ms: now_ms(),
        })
        .expect("resolve approval");

    let result = manager_task
        .await
        .expect("join manager task")
        .expect("manager result");
    assert_eq!(result.result.user_visible.as_deref(), Some("patch done"));

    let events = event_log.events.lock().expect("event log lock");
    assert!(events
        .iter()
        .any(|event| matches!(event, RuntimeEvent::ApprovalRequested(_))));
    assert!(events
        .iter()
        .any(|event| matches!(event, RuntimeEvent::ApprovalResolved(_))));
}
