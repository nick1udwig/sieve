use super::*;

#[tokio::test]
async fn composed_command_consolidates_single_approval() {
    let segments = vec![
        CommandSegment {
            argv: vec!["echo".to_string(), "hi".to_string()],
            operator_before: None,
        },
        CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
            operator_before: Some(sieve_types::CompositionOperator::And),
        },
    ];

    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::DenyWithApproval,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-1".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "echo hi && rm -rf tmp".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.command_segments.len(), 2);

    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::Deny,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(
        disposition,
        RuntimeDisposition::Denied {
            reason: "approval denied".to_string()
        }
    );
}

#[tokio::test]
async fn unknown_deny_and_accept_emit_policy_evaluated_events() {
    let segments = vec![CommandSegment {
        argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Unknown,
        segments,
        CommandKnowledge::Unknown,
        PolicyDecisionKind::Allow,
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
        .expect("runtime deny ok");
    assert_eq!(
        deny,
        RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string(),
        }
    );

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
        .expect("runtime accept ok");
    assert!(matches!(accept, RuntimeDisposition::ExecuteQuarantine(_)));
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[2], RuntimeEvent::QuarantineCompleted(_)));
}

#[tokio::test]
async fn uncertain_deny_and_accept_emit_policy_evaluated_events() {
    let segments = vec![CommandSegment {
        argv: vec!["weird-shell-construct".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Uncertain,
        segments,
        CommandKnowledge::Uncertain,
        PolicyDecisionKind::Allow,
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
        .expect("runtime deny ok");
    assert_eq!(
        deny,
        RuntimeDisposition::Denied {
            reason: "uncertain command denied by mode".to_string(),
        }
    );

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
        .expect("runtime accept ok");
    assert!(matches!(accept, RuntimeDisposition::ExecuteQuarantine(_)));
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[2], RuntimeEvent::QuarantineCompleted(_)));
}

#[tokio::test]
async fn unknown_ask_requires_approval_before_quarantine() {
    let segments = vec![CommandSegment {
        argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Unknown,
        segments,
        CommandKnowledge::Unknown,
        PolicyDecisionKind::Allow,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-1".to_string()),
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
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(
        disposition,
        RuntimeDisposition::ExecuteQuarantine(_)
    ));

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[2], RuntimeEvent::ApprovalResolved(_)));
    assert!(matches!(events[3], RuntimeEvent::QuarantineCompleted(_)));
}

#[tokio::test]
async fn unknown_ask_approve_always_skips_repeat_approval_for_same_command() {
    let segments = vec![CommandSegment {
        argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Unknown,
        segments,
        CommandKnowledge::Unknown,
        PolicyDecisionKind::Allow,
    );

    let first_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-1".to_string()),
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
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveAlways,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let first_disposition = first_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(
        first_disposition,
        RuntimeDisposition::ExecuteQuarantine(_)
    ));

    let second_disposition = timeout(
        Duration::from_millis(300),
        runtime.orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-2".to_string()),
            cwd: "/tmp".to_string(),
            script: "custom-cmd --flag".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Ask,
            uncertain_mode: UncertainMode::Deny,
        }),
    )
    .await
    .expect("second run should complete without waiting for approval")
    .expect("runtime ok");
    assert!(matches!(
        second_disposition,
        RuntimeDisposition::ExecuteQuarantine(_)
    ));
    assert_eq!(
        approval_bus
            .published_events()
            .expect("published approvals")
            .len(),
        1
    );
}

#[tokio::test]
async fn uncertain_ask_requires_approval_before_quarantine() {
    let segments = vec![CommandSegment {
        argv: vec!["weird-shell-construct".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Uncertain,
        segments,
        CommandKnowledge::Uncertain,
        PolicyDecisionKind::Allow,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-1".to_string()),
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

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "uncertain_command_mode");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(
        disposition,
        RuntimeDisposition::ExecuteQuarantine(_)
    ));

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[2], RuntimeEvent::ApprovalResolved(_)));
    assert!(matches!(events[3], RuntimeEvent::QuarantineCompleted(_)));
}

#[tokio::test]
async fn uncertain_ask_approve_always_skips_repeat_approval_for_same_command() {
    let segments = vec![CommandSegment {
        argv: vec!["weird-shell-construct".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Uncertain,
        segments,
        CommandKnowledge::Uncertain,
        PolicyDecisionKind::Allow,
    );

    let first_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-1".to_string()),
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

    let requested = wait_for_approval(&approval_bus).await;
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveAlways,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let first_disposition = first_task.await.expect("task join").expect("runtime ok");
    assert!(matches!(
        first_disposition,
        RuntimeDisposition::ExecuteQuarantine(_)
    ));

    let second_disposition = timeout(
        Duration::from_millis(300),
        runtime.orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-2".to_string()),
            cwd: "/tmp".to_string(),
            script: "weird-shell-construct".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Ask,
        }),
    )
    .await
    .expect("second run should complete without waiting for approval")
    .expect("runtime ok");
    assert!(matches!(
        second_disposition,
        RuntimeDisposition::ExecuteQuarantine(_)
    ));
    assert_eq!(
        approval_bus
            .published_events()
            .expect("published approvals")
            .len(),
        1
    );
}
