use super::*;

#[test]
fn approval_requested_event_schema_shape_stable() {
    let event = ApprovalRequestedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("approval-1".to_string()),
        run_id: RunId("run-1".to_string()),
        prompt_kind: sieve_types::ApprovalPromptKind::Command,
        title: None,
        command_segments: vec![CommandSegment {
            argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
            operator_before: None,
        }],
        inferred_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp".to_string(),
        }],
        blocked_rule_id: "rule-1".to_string(),
        reason: "requires approval".to_string(),
        preview: None,
        allow_approve_always: true,
        created_at_ms: 1000,
    };
    let as_json = serde_json::to_value(&event).expect("serialize");
    let obj = as_json.as_object().expect("event object");
    for key in [
        "schema_version",
        "request_id",
        "run_id",
        "prompt_kind",
        "title",
        "command_segments",
        "inferred_capabilities",
        "blocked_rule_id",
        "reason",
        "preview",
        "allow_approve_always",
        "created_at_ms",
    ] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
    assert_eq!(obj.len(), 12);
    assert_eq!(obj.get("schema_version"), Some(&Value::from(1)));
    assert_eq!(obj.get("request_id"), Some(&Value::from("approval-1")));
    assert_eq!(obj.get("run_id"), Some(&Value::from("run-1")));
}

#[test]
fn approval_resolved_event_schema_shape_stable() {
    let event = ApprovalResolvedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("approval-1".to_string()),
        run_id: RunId("run-1".to_string()),
        action: ApprovalAction::ApproveOnce,
        created_at_ms: 1001,
    };
    let as_json = serde_json::to_value(&event).expect("serialize");
    let obj = as_json.as_object().expect("event object");
    for key in [
        "schema_version",
        "request_id",
        "run_id",
        "action",
        "created_at_ms",
    ] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
    assert_eq!(obj.len(), 5);
    assert_eq!(obj.get("action"), Some(&Value::from("approve_once")));
}

#[tokio::test]
async fn approval_roundtrip_known_command() {
    let segments = vec![CommandSegment {
        argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
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
                    script: "rm -rf tmp".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let disposition = runtime_task.await.expect("task join").expect("runtime ok");
    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-1".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::PolicyEvaluated(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[2], RuntimeEvent::ApprovalResolved(_)));
    match &events[0] {
        RuntimeEvent::PolicyEvaluated(event) => assert_eq!(event.created_at_ms, 1000),
        _ => panic!("expected policy evaluated event"),
    }
    match &events[1] {
        RuntimeEvent::ApprovalRequested(event) => assert_eq!(event.created_at_ms, 1001),
        _ => panic!("expected approval requested event"),
    }
}

#[tokio::test]
async fn approval_bus_concurrent_requests_do_not_cross_resolve() {
    let segments = vec![CommandSegment {
        argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::DenyWithApproval,
    );

    let task_1 = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-1".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "rm -rf tmp".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };
    let task_2 = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-2".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "rm -rf tmp".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };

    let published = wait_for_approval_count(&approval_bus, 2).await;
    assert_eq!(published.len(), 2);
    let req_1 = published[0].clone();
    let req_2 = published[1].clone();
    assert_ne!(req_1.request_id, req_2.request_id);

    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: req_2.request_id.clone(),
            run_id: req_2.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 3000,
        })
        .expect("resolve second");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: req_1.request_id.clone(),
            run_id: req_1.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 3001,
        })
        .expect("resolve first");

    let out_1 = task_1.await.expect("join 1").expect("runtime 1");
    let out_2 = task_2.await.expect("join 2").expect("runtime 2");
    match out_1 {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-1".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }
    match out_2 {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-2".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }
}
