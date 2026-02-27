#![forbid(unsafe_code)]

mod common;

use common::{mk_runtime, wait_for_approval};
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_runtime::{RuntimeDisposition, ShellRunRequest};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{ApprovalAction, ApprovalResolvedEvent, RunId, UnknownMode};
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn approval_gate_allows_execution_after_approve_once() {
    let policy_toml = r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"
reason = "rm -rf requires approval"
"#;
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        Arc::new(common::RecordingQuarantine::default()),
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-approve".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "rm -rf /tmp/sieve-script-approve".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: sieve_types::UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1100,
        })
        .expect("resolve approval");

    let disposition = timeout(Duration::from_secs(2), runtime_task)
        .await
        .expect("runtime task timeout")
        .expect("runtime task join")
        .expect("runtime result");
    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-approve".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected ExecuteMainline, got {other:?}"),
    }
}

#[tokio::test]
async fn approval_gate_blocks_until_resolution() {
    let policy_toml = r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"
reason = "rm -rf requires approval"
"#;
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        Arc::new(common::RecordingQuarantine::default()),
    );

    let mut runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-requires-approval".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "rm -rf /tmp/sieve-script-requires-approval".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: sieve_types::UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "deny-rm-rf");

    assert!(
        timeout(Duration::from_millis(75), &mut runtime_task)
            .await
            .is_err(),
        "runtime completed before approval resolution"
    );

    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::Deny,
            created_at_ms: 1200,
        })
        .expect("resolve denial");

    let disposition = timeout(Duration::from_secs(2), runtime_task)
        .await
        .expect("runtime task timeout")
        .expect("runtime task join")
        .expect("runtime result");
    match disposition {
        RuntimeDisposition::Denied { reason } => {
            assert_eq!(reason, "approval denied");
        }
        other => panic!("expected Denied(approval denied), got {other:?}"),
    }
}
