use super::*;

#[tokio::test]
async fn endorse_request_lifecycle_uses_approval_flow() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_endorse_approval(
                    RunId("run-1".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v123".to_string()),
                        target_integrity: sieve_types::Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.command_segments[0].argv[0], "endorse");

    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::Deny,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let action = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(action, ApprovalAction::Deny);

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalResolved(_)));
}

#[tokio::test]
async fn endorse_request_deny_path_records_resolution() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_endorse_approval(
                    RunId("run-1".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v123".to_string()),
                        target_integrity: sieve_types::Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::Deny,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let action = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(action, ApprovalAction::Deny);
    let events = event_log.snapshot();
    match &events[1] {
        RuntimeEvent::ApprovalResolved(e) => assert_eq!(e.action, ApprovalAction::Deny),
        _ => panic!("expected approval resolved"),
    }
}

#[tokio::test]
async fn declassify_request_lifecycle_uses_approval_flow() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_declassify_approval(
                    RunId("run-1".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.command_segments[0].argv[0], "declassify");
    assert_eq!(
        requested.command_segments[0].argv[2],
        "https://api.example.com/v1/upload"
    );

    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let action = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(action, ApprovalAction::ApproveOnce);

    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::ApprovalRequested(_)));
    assert!(matches!(events[1], RuntimeEvent::ApprovalResolved(_)));
}

#[tokio::test]
async fn declassify_request_deny_path_records_resolution() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_declassify_approval(
                    RunId("run-1".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::Deny,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let action = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(action, ApprovalAction::Deny);
    let events = event_log.snapshot();
    assert!(matches!(events[0], RuntimeEvent::ApprovalRequested(_)));
    match &events[1] {
        RuntimeEvent::ApprovalResolved(e) => assert_eq!(e.action, ApprovalAction::Deny),
        _ => panic!("expected approval resolved"),
    }
}
