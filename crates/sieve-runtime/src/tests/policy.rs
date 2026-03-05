use super::*;

#[tokio::test]
async fn cp_summary_requires_capability_with_real_policy() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-1".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "cp src.txt dst.txt".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    match disposition {
        RuntimeDisposition::Denied { reason } => {
            assert!(reason.contains("missing capability"));
            assert!(reason.contains("dst.txt"));
        }
        other => panic!("expected denied disposition, got {other:?}"),
    }
}

#[tokio::test]
async fn cp_relative_destination_matches_absolute_policy_scope() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[[allow_capabilities]]
resource = "fs"
action = "write"
scope = "/tmp/workspace/dst.txt"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-2".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "cp src.txt ./out/../dst.txt".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-2".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }
}

#[tokio::test]
async fn curl_unsupported_flag_routes_to_unknown_mode_with_real_summary() {
    let (runtime, _approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-1".to_string()),
            cwd: "/tmp/workspace".to_string(),
            script: "curl -X POST -F file=@payload.bin https://api.example.com/v1/upload"
                .to_string(),
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
