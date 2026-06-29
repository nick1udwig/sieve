use super::*;

const POLICY: &str = r#"
[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://example.com/"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = false
"#;

#[tokio::test]
async fn sudo_agent_browser_contextual_sink_ref_uses_original_argv_index() {
    assert_wrapper_sink_ref(
        "run-ab-sudo-context",
        "sudo agent-browser --session demo fill '#q' secret",
        "argv:6",
    )
    .await;
}

#[tokio::test]
async fn env_agent_browser_contextual_sink_ref_uses_original_argv_index() {
    assert_wrapper_sink_ref(
        "run-ab-env-context",
        "env TOKEN=secret agent-browser --session demo fill '#q' secret",
        "argv:7",
    )
    .await;
}

#[tokio::test]
async fn optioned_wrappers_agent_browser_contextual_sink_ref_uses_original_argv_index() {
    assert_wrapper_sink_ref(
        "run-ab-optioned-context",
        "sudo -n -u root env -i TOKEN=secret agent-browser --session demo fill '#q' secret",
        "argv:12",
    )
    .await;
}

async fn assert_wrapper_sink_ref(run_id_prefix: &str, script: &str, expected_ref: &str) {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(POLICY);

    let first = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId(format!("{run_id_prefix}-1")),
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

    let second = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId(format!("{run_id_prefix}-2")),
            cwd: "/tmp/workspace".to_string(),
            script: script.to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("second run");

    match second {
        RuntimeDisposition::Denied { reason } => {
            assert!(reason.contains("cannot flow to sink"));
            assert!(reason.contains(expected_ref));
            assert!(reason.contains("https://example.com/"));
        }
        other => panic!("expected sink denial, got {other:?}"),
    }
}
