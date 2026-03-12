use super::*;
use sieve_types::SinkChannel;

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
    runtime
        .upsert_value_label(
            ValueRef("v123".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value state");

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
    runtime
        .upsert_value_label(
            ValueRef("v123".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value state");

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
    runtime
        .upsert_value_label(
            ValueRef("v456".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed value state");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_declassify_approval(
                    RunId("run-1".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        channel: SinkChannel::Body,
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
    assert_eq!(requested.command_segments[0].argv[3], "body");

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
    runtime
        .upsert_value_label(
            ValueRef("v456".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed value state");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_declassify_approval(
                    RunId("run-1".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        channel: SinkChannel::Body,
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

#[tokio::test]
async fn declassify_request_with_untrusted_control_uses_integrity_block_reason() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );
    runtime
        .upsert_value_label(
            ValueRef("v_control".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed untrusted control");
    runtime
        .upsert_value_label(
            ValueRef("v456".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed release value");
    let mut control_value_refs = BTreeSet::new();
    control_value_refs.insert(ValueRef("v_control".to_string()));

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .request_declassify_approval_with_context(
                    RunId("run-untrusted-control".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                        channel: SinkChannel::Body,
                        reason: None,
                    },
                    control_value_refs,
                    None,
                )
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "integrity-untrusted-control");
    assert_eq!(
        requested.reason,
        "untrusted control context for explicit tool action"
    );
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id,
            run_id: RunId("run-untrusted-control".to_string()),
            action: ApprovalAction::Deny,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let action = runtime_task.await.expect("task join").expect("runtime ok");
    assert_eq!(action, ApprovalAction::Deny);
}

#[tokio::test]
async fn endorse_request_trusted_string_is_denied_without_approval() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );
    runtime
        .upsert_value_label(
            ValueRef("v123".to_string()),
            ValueLabel {
                integrity: Integrity::Untrusted,
                provenance: BTreeSet::new(),
                allowed_sinks: BTreeSet::new(),
                capacity_type: sieve_types::CapacityType::TrustedString,
            },
        )
        .expect("seed trusted_string value");

    let action = runtime
        .request_endorse_approval(
            RunId("run-trusted-string-endorse".to_string()),
            EndorseRequest {
                value_ref: ValueRef("v123".to_string()),
                target_integrity: Integrity::Trusted,
                reason: None,
            },
        )
        .await
        .expect("runtime ok");

    assert_eq!(action, ApprovalAction::Deny);
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
}

#[tokio::test]
async fn declassify_request_trusted_string_is_denied_without_approval() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );
    runtime
        .upsert_value_label(
            ValueRef("v456".to_string()),
            ValueLabel {
                integrity: Integrity::Trusted,
                provenance: BTreeSet::new(),
                allowed_sinks: BTreeSet::new(),
                capacity_type: sieve_types::CapacityType::TrustedString,
            },
        )
        .expect("seed trusted_string value");

    let action = runtime
        .request_declassify_approval(
            RunId("run-trusted-string-declassify".to_string()),
            DeclassifyRequest {
                value_ref: ValueRef("v456".to_string()),
                sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                channel: SinkChannel::Body,
                reason: None,
            },
        )
        .await
        .expect("runtime ok");

    assert_eq!(action, ApprovalAction::Deny);
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
}

#[tokio::test]
async fn endorse_request_unknown_value_ref_errors_before_approval() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let err = runtime
        .request_endorse_approval(
            RunId("run-unknown-value".to_string()),
            EndorseRequest {
                value_ref: ValueRef("v-missing".to_string()),
                target_integrity: Integrity::Trusted,
                reason: None,
            },
        )
        .await
        .expect_err("unknown value ref must fail");

    match err {
        RuntimeError::ValueState(ValueStateError::UnknownValueRef(value_ref)) => {
            assert_eq!(value_ref, "v-missing");
        }
        other => panic!("expected unknown value ref error, got {other:?}"),
    }
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
}
