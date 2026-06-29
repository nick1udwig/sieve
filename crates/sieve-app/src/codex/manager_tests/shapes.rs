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
