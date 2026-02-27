#![forbid(unsafe_code)]

//! Paper case mapping (`papers/25-camel.md`) to current sieve runtime model.
//!
//! Covered here:
//! - Meeting-notes data-flow diversion (`§2`, `§3`, Figure 1/2) -> sink-flow denial/approval gate.
//! - Data-flow influencing consequential action (`§2`, `§6.4`) -> untrusted control requires approval.
//! - Human-in-the-loop policy enforcement (`§2`, `§5.2`) -> approval gate can allow one blocked run.
//!
//! Not directly representable in this runtime:
//! - Text-to-text/phishing non-goals (`§3.1`) require LLM/UI content semantics.
//!   Nearest enforceable invariant here: unknown command behavior is gated by approval before execution.

mod common;

use common::{
    label_with_sinks, mk_runtime, unique_temp_dir, wait_for_approval, RecordingQuarantine,
    StaticShell, StaticSummaries,
};
use sieve_command_summaries::{DefaultCommandSummarizer, SummaryOutcome};
use sieve_runtime::{RuntimeDisposition, ShellRunRequest};
use sieve_shell::{BasicShellAnalyzer, ShellAnalysis};
use sieve_types::{
    ApprovalAction, ApprovalResolvedEvent, CommandKnowledge, CommandSegment, Integrity, RunId,
    UncertainMode, UnknownMode, ValueRef,
};
use std::collections::BTreeSet;
use std::sync::Arc;

#[tokio::test]
async fn camel_case_meeting_notes_data_flow_diversion_denied() {
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
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed attacker-controlled payload");

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("paper-case-1".to_string()),
            cwd: "/tmp".to_string(),
            script: "curl -X POST https://api.example.com/v1/upload -d body".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    match disposition {
        RuntimeDisposition::Denied { reason } => {
            assert!(reason.contains("value argv:5 cannot flow to sink"));
            assert!(reason.contains("https://api.example.com/v1/upload"));
        }
        other => panic!("expected sink-flow denial, got {other:?}"),
    }
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
}

#[tokio::test]
async fn camel_case_meeting_notes_data_flow_diversion_requires_approval_in_ask_mode() {
    let policy_toml = r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

[options]
violation_mode = "ask"
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
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed attacker-controlled payload");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("paper-case-2".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "curl -X POST https://api.example.com/v1/upload -d body".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };
    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "sink-flow-denied");
    assert!(requested
        .reason
        .contains("value argv:5 cannot flow to sink"));
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1200,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("paper-case-2".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected approved mainline execution, got {other:?}"),
    }
}

#[tokio::test]
async fn camel_case_data_flow_to_control_requires_approval_for_mutating_action() {
    let root = unique_temp_dir("sieve-paper-case-control");
    let dst = root.join("dst.txt");
    let policy_toml = format!(
        r#"
[[allow_capabilities]]
resource = "fs"
action = "write"
scope = "{}"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        dst.to_string_lossy()
    );
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        &policy_toml,
        Arc::new(RecordingQuarantine::default()),
    );
    runtime
        .upsert_value_label(
            ValueRef("v-control".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed untrusted control");
    let mut control_value_refs = BTreeSet::new();
    control_value_refs.insert(ValueRef("v-control".to_string()));

    let runtime_task = {
        let runtime = runtime.clone();
        let cwd = root.to_string_lossy().to_string();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("paper-case-3".to_string()),
                    cwd,
                    script: "cp src.txt dst.txt".to_string(),
                    control_value_refs,
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };
    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "integrity-untrusted-control");
    assert_eq!(
        requested.reason,
        "untrusted control context for consequential action"
    );
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1300,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("paper-case-3".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected approved mainline execution, got {other:?}"),
    }
}

#[tokio::test]
async fn camel_case_non_goal_text_to_text_nearest_invariant_unknown_requires_approval() {
    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(RecordingQuarantine::default());
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(StaticShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Unknown,
                segments: vec![CommandSegment {
                    argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
                    operator_before: None,
                }],
                unsupported_constructs: Vec::new(),
            },
        }),
        Arc::new(StaticSummaries {
            outcome: SummaryOutcome {
                knowledge: CommandKnowledge::Unknown,
                summary: None,
                reason: None,
            },
        }),
        policy_toml,
        quarantine.clone(),
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("paper-case-4".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "custom-cmd --flag".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Ask,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "unknown_command_mode");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::Deny,
            created_at_ms: 1400,
        })
        .expect("resolve denial");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(
        disposition,
        RuntimeDisposition::Denied {
            reason: "approval denied".to_string(),
        }
    );
    assert!(quarantine.calls().is_empty());
}
