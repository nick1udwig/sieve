#![forbid(unsafe_code)]

mod common;

use common::{label_with_sinks, mk_runtime, wait_for_approval, wait_for_approval_count, RecordingQuarantine};
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_runtime::{RuntimeDisposition, ShellRunRequest};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{
    ApprovalAction, ApprovalResolvedEvent, EndorseRequest, Integrity, RunId, RuntimeEvent, SinkKey,
    UncertainMode, UnknownMode, ValueRef,
};
use std::collections::BTreeSet;
use std::sync::Arc;

#[tokio::test]
async fn rm_rf_is_gated_by_deny_with_approval() {
    let policy_toml = r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"
reason = "rm -rf requires approval"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(RecordingQuarantine::default());
    let (runtime, approval_bus, event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        quarantine,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-rm".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "rm -rf /tmp/demo".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "deny-rm-rf");
    assert_eq!(
        requested.command_segments[0].argv,
        vec!["rm".to_string(), "-rf".to_string(), "/tmp/demo".to_string()]
    );
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1100,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(disposition, RuntimeDisposition::ExecuteMainline);

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[2], RuntimeEvent::ApprovalResolved(_)));
}

#[tokio::test]
async fn curl_post_payload_sink_enforced_then_declassify_allows_flow() {
    let policy_toml = r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        Arc::new(RecordingQuarantine::default()),
    );
    runtime
        .upsert_value_label(
            ValueRef("argv:5".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed payload value");

    let first = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-curl-1".to_string()),
            cwd: "/tmp".to_string(),
            script: "curl -X POST https://api.example.com/v1/upload -d body".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    match first {
        RuntimeDisposition::Denied { reason } => {
            assert!(reason.contains("value argv:5 cannot flow to sink"));
            assert!(reason.contains("https://api.example.com/v1/upload"));
        }
        other => panic!("expected sink denial, got {other:?}"),
    }

    let declassify_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-declassify-1".to_string()),
                    sieve_types::DeclassifyRequest {
                        value_ref: ValueRef("argv:5".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };
    let first_requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: first_requested.request_id.clone(),
            run_id: first_requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1200,
        })
        .expect("resolve declassify approval");
    let first_transition = declassify_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert!(!first_transition.sink_was_already_allowed);

    let second = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-curl-2".to_string()),
            cwd: "/tmp".to_string(),
            script: "curl -X POST https://api.example.com/v1/upload -d body".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert_eq!(second, RuntimeDisposition::ExecuteMainline);

    let second_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-declassify-2".to_string()),
                    sieve_types::DeclassifyRequest {
                        value_ref: ValueRef("argv:5".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };
    let second_requested = wait_for_approval_count(&approval_bus, 2).await[1].clone();
    assert_ne!(first_requested.request_id, second_requested.request_id);
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: second_requested.request_id,
            run_id: second_requested.run_id,
            action: ApprovalAction::Deny,
            created_at_ms: 1201,
        })
        .expect("resolve second declassify approval");
    let second_transition = second_task
        .await
        .expect("task join")
        .expect("runtime ok");
    assert!(second_transition.is_none());
}

#[tokio::test]
async fn endorse_requires_fresh_approval_each_invocation() {
    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        Arc::new(RecordingQuarantine::default()),
    );
    runtime
        .upsert_value_label(
            ValueRef("v-control".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value label");

    let first_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .endorse_value_once(
                    RunId("run-endorse-1".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v-control".to_string()),
                        target_integrity: Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };
    let first_requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: first_requested.request_id.clone(),
            run_id: first_requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1400,
        })
        .expect("resolve first endorse");
    let first_transition = first_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert_eq!(first_transition.to_integrity, Integrity::Trusted);

    let second_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .endorse_value_once(
                    RunId("run-endorse-2".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v-control".to_string()),
                        target_integrity: Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };
    let second_requested = wait_for_approval_count(&approval_bus, 2).await[1].clone();
    assert_ne!(first_requested.request_id, second_requested.request_id);
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: second_requested.request_id,
            run_id: second_requested.run_id,
            action: ApprovalAction::Deny,
            created_at_ms: 1401,
        })
        .expect("resolve second endorse");
    let second_transition = second_task
        .await
        .expect("task join")
        .expect("runtime ok");
    assert!(second_transition.is_none());
}
