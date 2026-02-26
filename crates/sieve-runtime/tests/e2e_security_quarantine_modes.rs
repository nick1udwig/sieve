#![forbid(unsafe_code)]

mod common;

use common::{
    mk_runtime, unique_temp_dir, wait_for_approval_count, write_fake_bwrap, RecordingQuarantine,
    StaticShell, StaticSummaries,
};
use sieve_command_summaries::{DefaultCommandSummarizer, SummaryOutcome};
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{RuntimeDisposition, ShellRunRequest};
use sieve_shell::{BasicShellAnalyzer, ShellAnalysis};
use sieve_types::{
    ApprovalAction, ApprovalResolvedEvent, CommandKnowledge, CommandSegment, RunId, UncertainMode,
    UnknownMode,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::test]
async fn unknown_modes_cover_deny_accept_and_ask() {
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

    let deny = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-unknown-deny".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert_eq!(
        deny,
        RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string(),
        }
    );
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert!(quarantine.calls().is_empty());

    let accept = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-unknown-accept".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Accept,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert!(matches!(accept, RuntimeDisposition::ExecuteQuarantine(_)));
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert_eq!(quarantine.calls().len(), 1);

    let ask_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-unknown-ask".to_string()),
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
    let requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    assert_eq!(requested.blocked_rule_id, "unknown_command_mode");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1300,
        })
        .expect("resolve unknown ask approval");
    let ask = ask_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(ask, RuntimeDisposition::ExecuteQuarantine(_)));
    assert_eq!(quarantine.calls().len(), 2);
}

#[tokio::test]
async fn uncertain_modes_cover_deny_accept_and_ask() {
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
                knowledge: CommandKnowledge::Uncertain,
                segments: vec![CommandSegment {
                    argv: vec!["weird-shell-construct".to_string()],
                    operator_before: None,
                }],
                unsupported_constructs: vec!["substitution_or_expansion".to_string()],
            },
        }),
        Arc::new(StaticSummaries {
            outcome: SummaryOutcome {
                knowledge: CommandKnowledge::Uncertain,
                summary: None,
                reason: Some("unsupported shell construct".to_string()),
            },
        }),
        policy_toml,
        quarantine.clone(),
    );

    let deny = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-uncertain-deny".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert_eq!(
        deny,
        RuntimeDisposition::Denied {
            reason: "uncertain command denied by mode".to_string(),
        }
    );
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert!(quarantine.calls().is_empty());

    let accept = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-uncertain-accept".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Accept,
        })
        .await
        .expect("runtime ok");
    assert!(matches!(accept, RuntimeDisposition::ExecuteQuarantine(_)));
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    assert_eq!(quarantine.calls().len(), 1);

    let ask_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-uncertain-ask".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "weird-shell-construct".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Ask,
                })
                .await
        })
    };
    let requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    assert_eq!(requested.blocked_rule_id, "uncertain_command_mode");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: requested.run_id,
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1301,
        })
        .expect("resolve uncertain ask approval");
    let ask = ask_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(ask, RuntimeDisposition::ExecuteQuarantine(_)));
    assert_eq!(quarantine.calls().len(), 2);
}

#[tokio::test]
async fn unknown_accept_path_generates_quarantine_report_json() {
    let root = unique_temp_dir("sieve-runtime-e2e");
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    let fake_bwrap = bin_dir.join("fake-bwrap");
    write_fake_bwrap(&fake_bwrap);

    let policy_toml = r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#;
    let quarantine = Arc::new(BwrapQuarantineRunner::with_programs(
        root.join(".sieve/logs/traces"),
        fake_bwrap.to_string_lossy().to_string(),
        "strace",
        "bash",
    ));
    let (runtime, approval_bus, _event_log) = mk_runtime(
        Arc::new(BasicShellAnalyzer),
        Arc::new(DefaultCommandSummarizer),
        policy_toml,
        quarantine,
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-quarantine".to_string()),
            cwd: "/".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Accept,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());

    let report = match disposition {
        RuntimeDisposition::ExecuteQuarantine(report) => report,
        other => panic!("expected quarantine disposition, got {other:?}"),
    };
    let report_json_path = PathBuf::from(&report.trace_path).join("report.json");
    let report_json = fs::read_to_string(&report_json_path).expect("report json");
    assert!(report_json.contains("\"run_id\": \"run-quarantine\""));
    assert!(report_json.contains("\"trace_files\": ["));
    assert!(report_json.contains("strace.123"));
    assert!(report_json.contains("\"attempted_capabilities\": ["));

    fs::remove_dir_all(root).expect("cleanup");
}
