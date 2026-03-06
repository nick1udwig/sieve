use super::*;

#[tokio::test]
async fn agent_browser_chain_assumes_implicit_session_after_open() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://example.com/"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = false
"#,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-chain".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser open https://example.com && agent-browser snapshot -i"
                .to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    assert!(matches!(
        disposition,
        RuntimeDisposition::ExecuteMainline(_)
    ));
}

#[tokio::test]
async fn agent_browser_named_session_persists_across_turns() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://example.com/"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = false
"#,
    );

    let first = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-session-1".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser --session demo open https://example.com".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("first run");
    assert!(matches!(first, RuntimeDisposition::ExecuteMainline(_)));

    let sessions = runtime
        .browser_sessions_snapshot()
        .expect("browser sessions snapshot");
    let demo = sessions.get("demo").expect("demo session recorded");
    assert_eq!(demo.current_origin, "https://example.com/");
    assert_eq!(demo.current_url, "https://example.com/");

    let second = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-session-2".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser --session demo snapshot -i".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("second run");
    assert!(matches!(second, RuntimeDisposition::ExecuteMainline(_)));
}

#[tokio::test]
async fn agent_browser_hidden_page_command_without_session_is_denied() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = false
"#,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-no-session".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser snapshot -i".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    assert_eq!(
        disposition,
        RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string()
        }
    );
}

#[tokio::test]
async fn agent_browser_unknown_named_session_is_denied() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = false
"#,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-missing-session".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser --session missing snapshot -i".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    assert_eq!(
        disposition,
        RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string()
        }
    );
}

#[tokio::test]
async fn agent_browser_close_clears_named_session() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://example.com/"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = false
"#,
    );

    let first = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-close-1".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser --session demo open https://example.com".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("open run");
    assert!(matches!(first, RuntimeDisposition::ExecuteMainline(_)));

    let second = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-close-2".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser --session demo close".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("close run");
    assert!(matches!(second, RuntimeDisposition::ExecuteMainline(_)));
    assert!(runtime
        .browser_sessions_snapshot()
        .expect("browser sessions snapshot")
        .is_empty());

    let third = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-ab-close-3".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "agent-browser --session demo snapshot -i".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("snapshot run");
    assert_eq!(
        third,
        RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string()
        }
    );
}
