use super::*;
use sieve_runtime::{InProcessApprovalBus, RuntimeEventLog};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::time::Instant;
use tokio::time::sleep;

#[test]
fn sandbox_policy_workspace_write_includes_cwd_and_roots() {
    let value = serde_json::to_value(sandbox_policy_json(
        CodexSandboxMode::WorkspaceWrite,
        Some("/repo"),
        &["/repo/subdir".to_string()],
    ))
    .expect("sandbox policy");
    assert_eq!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("workspaceWrite")
    );
    let roots = value
        .get("writableRoots")
        .and_then(serde_json::Value::as_array)
        .expect("roots array");
    assert_eq!(roots.len(), 2);
    assert!(value.get("readOnlyAccess").is_none());
}

#[test]
fn sandbox_policy_read_only_matches_app_server_shape() {
    let value = serde_json::to_value(sandbox_policy_json(
        CodexSandboxMode::ReadOnly,
        Some("/repo"),
        &[],
    ))
    .expect("sandbox policy");
    assert_eq!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some("readOnly")
    );
    assert_eq!(
        value
            .get("networkAccess")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert!(value.get("access").is_none());
    assert!(value.get("readOnlyAccess").is_none());
}

#[test]
fn decode_turn_result_uses_structured_payload() {
    let result = decode_turn_result(
        Some("session-1".to_string()),
        "fix-auth-flow".to_string(),
        Some("turn-1".to_string()),
        Some("thread-1".to_string()),
        r#"{"status":"needs_followup","summary":"implemented phase 1","user_visible":null}"#,
        Some("completed"),
        None,
    );
    assert_eq!(result.status, CodexTurnStatus::NeedsFollowup);
    assert_eq!(result.summary, "implemented phase 1");
    assert!(result.user_visible.is_none());
}

#[test]
fn decode_command_exec_result_uses_raw_stdout_and_stderr() {
    let result = decode_command_exec_result(&serde_json::json!({
        "exitCode": 7,
        "stdout": "ok",
        "stderr": "warn",
    }))
    .expect("command exec result");
    assert_eq!(result.exit_code, 7);
    assert_eq!(result.stdout, "ok");
    assert_eq!(result.stderr, "warn");
}

#[test]
fn handle_item_notification_extracts_file_change_preview() {
    let mut state = TurnStreamState::default();
    handle_item_notification(
        &serde_json::json!({
            "params": {
                "item": {
                    "type": "fileChange",
                    "id": "patch-1",
                    "changes": [
                        {
                            "path": "src/main.rs",
                            "diff": "@@ -1 +1 @@\n-old\n+new"
                        }
                    ]
                }
            }
        }),
        &mut state,
    );
    assert!(state
        .file_change_previews
        .get("patch-1")
        .is_some_and(|value| value.contains("src/main.rs")));
}

#[test]
fn truncate_preview_preserves_existing_preview_shape() {
    assert_eq!(truncate_preview("abc", 3), "abc");
    assert_eq!(truncate_preview("abcd", 3), "abc\n...[truncated]");
    assert_eq!(truncate_preview("éclair", 1), "é\n...[truncated]");
    assert_eq!(truncate_preview("abc", 0), "\n...[truncated]");
}

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

#[test]
fn build_turn_input_includes_bounded_increment_instruction() {
    let input = build_turn_input("build the app", &[]);
    let input = serde_json::to_value(input).expect("turn input json");
    let text = input[0]
        .get("text")
        .and_then(serde_json::Value::as_str)
        .expect("turn input text");
    assert!(text.contains("bounded increment"));
    assert!(text.contains("prefer `needs_followup`"));
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

#[derive(Default)]
struct RecordingEventLog {
    events: Mutex<Vec<RuntimeEvent>>,
}

#[async_trait]
impl RuntimeEventLog for RecordingEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), sieve_runtime::EventLogError> {
        self.events.lock().expect("event log lock").push(event);
        Ok(())
    }
}

fn test_manager(
    root: &Path,
    script: &Path,
    approval_bus: Arc<InProcessApprovalBus>,
    event_log: Arc<RecordingEventLog>,
) -> CodexManager {
    CodexManager {
        config: CodexManagerConfig {
            program: Some(script.to_string_lossy().to_string()),
            ws_url: None,
            connect_timeout_ms: DEFAULT_CODEX_CONNECT_TIMEOUT_MS,
            model: Some("mock-model".to_string()),
            turn_timeout_ms: 5_000,
            request_timeout_ms: 5_000,
        },
        store: CodexSessionStore::new(root.join("state/codex.db")).expect("create codex store"),
        approval_bus,
        event_log,
        active_sessions: Mutex::new(HashMap::new()),
    }
}

async fn wait_for_active_planner_session(manager: &CodexManager) -> PlannerCodexSession {
    for _ in 0..200 {
        if let Ok(sessions) = manager.planner_sessions().await {
            for session in sessions {
                let is_active = manager
                    .active_sessions
                    .lock()
                    .expect("active session lock")
                    .contains_key(&session.session_id);
                if is_active {
                    return session;
                }
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("planner session was not active");
}

async fn wait_for_published_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
    for _ in 0..200 {
        if let Ok(events) = bus.published_events() {
            if let Some(event) = events.last() {
                return event.clone();
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("approval was not published");
}

async fn auto_approve_codex_requests(bus: Arc<InProcessApprovalBus>) {
    let mut resolved = HashSet::new();
    loop {
        if let Ok(events) = bus.published_events() {
            for event in events {
                if !resolved.insert(event.request_id.clone()) {
                    continue;
                }
                let action = match event.prompt_kind {
                    ApprovalPromptKind::Command => ApprovalAction::ApproveOnce,
                    ApprovalPromptKind::FileChange => ApprovalAction::ApproveAlways,
                };
                let _ = bus.resolve(ApprovalResolvedEvent {
                    schema_version: 1,
                    request_id: event.request_id.clone(),
                    run_id: event.run_id.clone(),
                    action,
                    created_at_ms: now_ms(),
                });
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn write_mock_server(root: &Path, body: &str) -> PathBuf {
    let path = root.join(format!("mock-codex-server-{}.py", Uuid::new_v4()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create mock server dir");
    }
    let mut file = fs::File::create(&path).expect("create mock server");
    file.write_all(body.as_bytes()).expect("write mock server");
    let mut permissions = file.metadata().expect("mock server metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod mock server");
    path
}

fn unique_test_root(prefix: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("create test root");
    root
}
