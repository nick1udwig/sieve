#[tokio::test]
async fn codex_manager_session_handles_pending_notifications_between_requests() {
    let root = unique_test_root("codex-pending-notifications");
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
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_pending"}}}), flush=True)
        print(json.dumps({"method": "thread/started", "params": {"thread": {"id": "thr_pending"}}}), flush=True)
        print(json.dumps({"method": "codex/event/mcp_startup_complete", "params": {"id": "", "msg": {"type": "mcp_startup_complete"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_pending"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_pending", "text": "{\"status\":\"completed\",\"summary\":\"pending notifications handled\",\"user_visible\":\"pending ok\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_pending", "threadId": "thr_pending", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(&root, &script, approval_bus, event_log);

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        manager.run_session(CodexSessionRequest {
            session_id: None,
            instruction: "start the project".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            local_images: Vec::new(),
        }),
    )
    .await
    .expect("manager should not spin forever")
    .expect("codex session");

    assert_eq!(result.result.user_visible.as_deref(), Some("pending ok"));
}
#[tokio::test]
async fn codex_manager_session_times_out_hung_startup_request() {
    let root = unique_test_root("codex-startup-timeout");
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
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_timeout"}}}), flush=True)
    elif method == "thread/name/set":
        continue
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = CodexManager {
        config: CodexManagerConfig {
            program: Some(script.to_string_lossy().to_string()),
            ws_url: None,
            connect_timeout_ms: DEFAULT_CODEX_CONNECT_TIMEOUT_MS,
            model: Some("mock-model".to_string()),
            turn_timeout_ms: 5_000,
            request_timeout_ms: 100,
        },
        store: CodexSessionStore::new(root.join("state/codex.db")).expect("create codex store"),
        approval_bus,
        event_log,
        active_sessions: Mutex::new(HashMap::new()),
    };

    let started = Instant::now();
    let err = tokio::time::timeout(
        Duration::from_secs(2),
        manager.run_session(CodexSessionRequest {
            session_id: None,
            instruction: "start the project".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            local_images: Vec::new(),
        }),
    )
    .await
    .expect("manager should not hang")
    .expect_err("startup timeout should surface as error");

    assert!(
        err.contains("thread/name/set timed out after 100ms"),
        "unexpected startup timeout error: {err}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
#[tokio::test]
async fn codex_manager_live_session_smoke_real_prompt() {
    if env::var("SIEVE_RUN_CODEX_LIVE").ok().as_deref() != Some("1") {
        return;
    }

    let root = unique_test_root("codex-live-modex");
    let repo = root.join("modex");
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = Arc::new(CodexManager {
        config: CodexManagerConfig {
            program: env::var("SIEVE_CODEX_APP_SERVER_BIN")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| Some("codex".to_string())),
            ws_url: env::var("SIEVE_CODEX_APP_SERVER_WS_URL")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            connect_timeout_ms: env::var("SIEVE_CODEX_APP_SERVER_CONNECT_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(DEFAULT_CODEX_CONNECT_TIMEOUT_MS),
            model: env::var("SIEVE_CODEX_MODEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| Some("gpt-5.4".to_string())),
            turn_timeout_ms: 10 * 60 * 1000,
            request_timeout_ms: 30 * 1000,
        },
        store: CodexSessionStore::new(root.join("state/codex.db")).expect("create codex store"),
        approval_bus: approval_bus.clone(),
        event_log,
        active_sessions: Mutex::new(HashMap::new()),
    });

    let prompt = format!(
            "create a new project in {}\n\nit is going to be a mobile codex interface. the codex app-server runs on a vps and then this is a webapp that talks to the app-server\n\nit exposes a chat interface just like codex cli or the chatgpt mobile app. but in addition to have a \"list\" of chats, it also has \"browser tabs\" of chats (similar to any browser os when you open the tabs view) that can be \"closed\" (they can always be accessed in the sidebar). when a chat is \"opened\" its opened like a new tab. tabs indicate when they are running or not running with a little icon\n\nstart a codex session to start building it",
            repo.display()
        );
    let manager_task = {
        let manager = manager.clone();
        let repo_string = repo.to_string_lossy().to_string();
        tokio::spawn(async move {
            manager
                .run_session(CodexSessionRequest {
                    session_id: None,
                    instruction: prompt,
                    sandbox: CodexSandboxMode::WorkspaceWrite,
                    cwd: Some(repo_string.clone()),
                    writable_roots: vec![repo_string],
                    local_images: Vec::new(),
                })
                .await
        })
    };

    let approvals_task = tokio::spawn(auto_approve_codex_requests(approval_bus.clone()));

    let result = tokio::time::timeout(Duration::from_secs(12 * 60), manager_task)
        .await
        .expect("live codex session timed out")
        .expect("join live codex session")
        .expect("live codex session result");

    approvals_task.abort();

    assert!(
        result.result.session_id.is_some(),
        "expected persistent codex session id"
    );
    assert!(
        result.result.thread_id.is_some(),
        "expected codex thread id from live run"
    );
    assert!(
        !result.result.summary.trim().is_empty(),
        "expected non-empty summary from live run"
    );
    assert!(
        repo.exists(),
        "expected live codex run to create repo directory at {}",
        repo.display()
    );
}
#[tokio::test]
async fn codex_manager_persists_and_resumes_session_threads() {
    let root = unique_test_root("codex-session");
    let first_script = write_mock_server(
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
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_saved"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_saved_1"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_1", "text": "{\"status\":\"completed\",\"summary\":\"phase one done\",\"user_visible\":null}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_saved_1", "threadId": "thr_saved", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(
        &root,
        &first_script,
        approval_bus.clone(),
        event_log.clone(),
    );

    let first = manager
        .run_session(CodexSessionRequest {
            session_id: None,
            instruction: "implement phase one".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            local_images: Vec::new(),
        })
        .await
        .expect("first session run");

    let session_id = first
        .result
        .session_id
        .clone()
        .expect("persistent session id");
    let stored = manager.planner_sessions().await.expect("planner sessions");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].session_id, session_id);
    assert_eq!(stored[0].status, "completed");

    let second_script = write_mock_server(
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
    elif method == "thread/resume":
        assert msg["params"]["threadId"] == "thr_saved"
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_saved"}}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_saved_2"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_2", "text": "{\"status\":\"needs_followup\",\"summary\":\"phase two started\",\"user_visible\":null}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_saved_2", "threadId": "thr_saved", "status": "completed"}}}), flush=True)
"#,
    );
    let manager = test_manager(&root, &second_script, approval_bus, event_log);
    let second = manager
        .run_session(CodexSessionRequest {
            session_id: Some(session_id.clone()),
            instruction: "continue phase two".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            local_images: Vec::new(),
        })
        .await
        .expect("second session run");

    assert_eq!(second.result.status, CodexTurnStatus::NeedsFollowup);
    let stored = manager.planner_sessions().await.expect("planner sessions");
    assert_eq!(stored[0].session_id, session_id);
    assert_eq!(stored[0].status, "needs_followup");
    assert_eq!(
        stored[0].last_result_summary.as_deref(),
        Some("phase two started")
    );
}
#[tokio::test]
async fn codex_manager_restarts_missing_rollout_with_new_thread() {
    let root = unique_test_root("codex-missing-rollout");
    let first_script = write_mock_server(
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
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_saved_old"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_saved_old"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_old", "text": "{\"status\":\"completed\",\"summary\":\"phase one done\",\"user_visible\":null}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_saved_old", "threadId": "thr_saved_old", "status": "completed"}}}), flush=True)
"#,
    );
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(RecordingEventLog::default());
    let manager = test_manager(
        &root,
        &first_script,
        approval_bus.clone(),
        event_log.clone(),
    );
    let first = manager
        .run_session(CodexSessionRequest {
            session_id: None,
            instruction: "implement phase one".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            local_images: Vec::new(),
        })
        .await
        .expect("first session run");
    let session_id = first
        .result
        .session_id
        .clone()
        .expect("persistent session id");

    let second_script = write_mock_server(
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
    elif method == "thread/resume":
        assert msg["params"]["threadId"] == "thr_saved_old"
        print(json.dumps({"id": msg["id"], "error": {"message": "no rollout found for thread id thr_saved_old"}}), flush=True)
    elif method == "thread/start":
        assert msg["params"]["cwd"] == "/tmp/repo"
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_saved_new"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        assert msg["params"]["cwd"] == "/tmp/repo"
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_saved_new"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_new", "text": "{\"status\":\"completed\",\"summary\":\"restarted on fresh thread\",\"user_visible\":\"restarted\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_saved_new", "threadId": "thr_saved_new", "status": "completed"}}}), flush=True)
"#,
    );
    let manager = test_manager(&root, &second_script, approval_bus, event_log);
    let second = manager
        .run_session(CodexSessionRequest {
            session_id: Some(session_id.clone()),
            instruction: "continue phase two".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: vec!["/tmp/repo".to_string()],
            local_images: Vec::new(),
        })
        .await
        .expect("second session run");

    assert_eq!(second.result.user_visible.as_deref(), Some("restarted"));
    let stored = manager.planner_sessions().await.expect("planner sessions");
    assert_eq!(stored[0].session_id, session_id);
    let persisted = manager
        .store
        .session(&stored[0].session_id)
        .expect("load stored session")
        .expect("stored session row");
    assert_eq!(persisted.thread_id, "thr_saved_new");
}
