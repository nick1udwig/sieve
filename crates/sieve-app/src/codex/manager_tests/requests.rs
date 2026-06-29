#[tokio::test]
async fn codex_manager_exec_uses_command_exec_with_requested_sandbox() {
    let root = unique_test_root("codex-command-exec");
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
    elif method == "command/exec":
        params = msg["params"]
        assert params["command"] == ["git", "status"]
        assert params["cwd"] == "/tmp/repo"
        assert params["timeoutMs"] == 5000
        assert params["sandboxPolicy"]["type"] == "workspaceWrite"
        assert params["sandboxPolicy"]["networkAccess"] is False
        assert "/tmp/repo" in params["sandboxPolicy"]["writableRoots"]
        print(json.dumps({"id": msg["id"], "result": {"exitCode": 0, "stdout": "clean", "stderr": ""}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(&root, &script, approval_bus, event_log);

    let result = manager
        .exec(CodexExecRequest {
            command: vec!["git".to_string(), "status".to_string()],
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            timeout_ms: Some(5000),
        })
        .await
        .expect("codex exec");

    assert_eq!(result.result.exit_code, 0);
    assert_eq!(result.result.stdout, "clean");
    assert_eq!(result.result.stderr, "");
}
#[tokio::test]
async fn codex_manager_session_normalizes_tilde_paths_before_turn_start() {
    let root = unique_test_root("codex-session-paths");
    let (expected, request) = {
        let _env_guard = crate::tests::env_test_lock()
            .lock()
            .expect("env test lock poisoned");
        let expected = format!(
            "{}/git/modex",
            env::var("HOME").unwrap_or_else(|_| "/root".to_string())
        );
        let request = normalize_session_request(CodexSessionRequest {
            session_id: None,
            instruction: "start the mobile codex project".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("~/git/modex".to_string()),
            writable_roots: vec!["~/git/modex".to_string()],
            local_images: Vec::new(),
        })
        .expect("normalize session request");
        assert_eq!(request.cwd.as_deref(), Some(expected.as_str()));
        assert_eq!(request.writable_roots, vec![expected.clone()]);
        (expected, request)
    };
    let script = write_mock_server(
        &root,
        &format!(
            r#"#!/usr/bin/env python3
import json, sys
expected = {expected:?}
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({{"id": msg["id"], "result": {{"ok": True}}}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/start":
        assert msg["params"]["cwd"] == expected
        print(json.dumps({{"id": msg["id"], "result": {{"thread": {{"id": "thr_paths"}}}}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({{"id": msg["id"], "result": {{}}}}), flush=True)
    elif method == "turn/start":
        params = msg["params"]
        assert params["cwd"] == expected
        assert expected in params["sandboxPolicy"]["writableRoots"]
        assert all(path.startswith("/") for path in params["sandboxPolicy"]["writableRoots"])
        print(json.dumps({{"id": msg["id"], "result": {{"turn": {{"id": "turn_paths"}}}}}}), flush=True)
        print(json.dumps({{"method": "item/completed", "params": {{"item": {{"type": "agentMessage", "id": "msg_paths", "text": "{{\\\"status\\\":\\\"completed\\\",\\\"summary\\\":\\\"paths ok\\\",\\\"user_visible\\\":\\\"paths ok\\\"}}"}}}}}}), flush=True)
        print(json.dumps({{"method": "turn/completed", "params": {{"turn": {{"id": "turn_paths", "threadId": "thr_paths", "status": "completed"}}}}}}), flush=True)
"#,
            expected = expected
        ),
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(&root, &script, approval_bus, event_log);

    let result = manager.run_session(request).await.expect("codex session");

    assert_eq!(result.result.status, CodexTurnStatus::Completed);
    assert!(result.result.summary.contains("paths ok"));
}
#[tokio::test]
async fn codex_manager_task_passes_local_image_input() {
    let root = unique_test_root("codex-exec");
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
        assert msg["params"]["ephemeral"] is True
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_exec"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        params = msg["params"]
        assert params["sandboxPolicy"]["type"] == "readOnly"
        assert params["sandboxPolicy"]["networkAccess"] is False
        assert any(entry["type"] == "localImage" and entry["path"].endswith("image-input.png") for entry in params["input"])
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_exec"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_1", "text": "{\"status\":\"completed\",\"summary\":\"ocr ok\",\"user_visible\":\"ocr text\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_exec", "threadId": "thr_exec", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(&root, &script, approval_bus, event_log);

    let result = manager
        .run_task(CodexSessionRequest {
            session_id: None,
            instruction: "read the screenshot".to_string(),
            sandbox: CodexSandboxMode::ReadOnly,
            cwd: None,
            writable_roots: Vec::new(),
            local_images: vec![root.join("image-input.png").to_string_lossy().to_string()],
        })
        .await
        .expect("codex task");

    assert_eq!(result.result.status, CodexTurnStatus::Completed);
    assert_eq!(result.result.user_visible.as_deref(), Some("ocr text"));
}
#[tokio::test]
async fn codex_manager_session_uses_kebab_case_approval_policy() {
    let root = unique_test_root("codex-approval-policy");
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
        assert msg["params"]["approvalPolicy"] == "on-request"
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_policy"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        assert msg["params"]["approvalPolicy"] == "on-request"
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_policy"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_policy", "text": "{\"status\":\"completed\",\"summary\":\"policy ok\",\"user_visible\":\"policy ok\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_policy", "threadId": "thr_policy", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(&root, &script, approval_bus, event_log);

    let result = manager
        .run_session(CodexSessionRequest {
            session_id: None,
            instruction: "inspect the repo".to_string(),
            sandbox: CodexSandboxMode::ReadOnly,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: Vec::new(),
            local_images: Vec::new(),
        })
        .await
        .expect("codex session");

    assert_eq!(result.result.user_visible.as_deref(), Some("policy ok"));
}
