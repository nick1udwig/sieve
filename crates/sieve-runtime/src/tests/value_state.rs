use super::*;

#[tokio::test]
async fn endorse_value_once_updates_runtime_state_when_approved() {
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
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value state");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .endorse_value_once(
                    RunId("run-1".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v123".to_string()),
                        target_integrity: Integrity::Trusted,
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
            request_id: requested.request_id.clone(),
            run_id: RunId("run-1".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let transition = runtime_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert_eq!(transition.value_ref, ValueRef("v123".to_string()));
    assert_eq!(transition.from_integrity, Integrity::Untrusted);
    assert_eq!(transition.to_integrity, Integrity::Trusted);
    assert_eq!(transition.approved_by, Some(requested.request_id));

    let label = runtime
        .value_label(&ValueRef("v123".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert_eq!(label.integrity, Integrity::Trusted);
}

#[tokio::test]
async fn declassify_value_once_tracks_existing_sink_allowance() {
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
    let sink = "https://api.example.com/v1/upload";
    runtime
        .upsert_value_label(
            ValueRef("v456".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed value state");

    let first_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-1".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey(sink.to_string()),
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
            created_at_ms: 2000,
        })
        .expect("resolve first approval");
    let first_transition = first_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert!(!first_transition.sink_was_already_allowed);

    let second_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-2".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey(sink.to_string()),
                        reason: None,
                    },
                )
                .await
        })
    };
    let second_requested = wait_for_approval_count(&approval_bus, 2).await[1].clone();
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: second_requested.request_id.clone(),
            run_id: second_requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2001,
        })
        .expect("resolve second approval");
    let second_transition = second_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert!(second_transition.sink_was_already_allowed);

    let label = runtime
        .value_label(&ValueRef("v456".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert!(label.allowed_sinks.contains(&SinkKey(sink.to_string())));
}

#[tokio::test]
async fn endorse_value_once_policy_deny_skips_approval_and_transition() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Deny,
    );
    runtime
        .upsert_value_label(
            ValueRef("v123".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value state");

    let transition = runtime
        .endorse_value_once(
            RunId("run-1".to_string()),
            EndorseRequest {
                value_ref: ValueRef("v123".to_string()),
                target_integrity: Integrity::Trusted,
                reason: None,
            },
        )
        .await
        .expect("runtime ok");

    assert!(transition.is_none());
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    let label = runtime
        .value_label(&ValueRef("v123".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert_eq!(label.integrity, Integrity::Untrusted);
}

#[tokio::test]
async fn endorse_value_once_policy_deny_with_approval_uses_policy_metadata() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::DenyWithApproval,
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
                .endorse_value_once(
                    RunId("run-1".to_string()),
                    EndorseRequest {
                        value_ref: ValueRef("v123".to_string()),
                        target_integrity: Integrity::Trusted,
                        reason: None,
                    },
                )
                .await
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.blocked_rule_id, "rule-1");
    assert_eq!(requested.reason, "policy verdict");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2000,
        })
        .expect("resolve approval");

    let transition = runtime_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");
    assert_eq!(transition.value_ref, ValueRef("v123".to_string()));
    assert_eq!(transition.approved_by, Some(requested.request_id));
}

#[tokio::test]
async fn declassify_value_once_policy_deny_skips_approval_and_transition() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string()],
        operator_before: None,
    }];
    let (runtime, approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Deny,
    );
    let sink = SinkKey("https://api.example.com/v1/upload".to_string());
    runtime
        .upsert_value_label(
            ValueRef("v456".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed value state");

    let transition = runtime
        .declassify_value_once(
            RunId("run-1".to_string()),
            DeclassifyRequest {
                value_ref: ValueRef("v456".to_string()),
                sink: sink.clone(),
                reason: None,
            },
        )
        .await
        .expect("runtime ok");

    assert!(transition.is_none());
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    let label = runtime
        .value_label(&ValueRef("v456".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert!(!label.allowed_sinks.contains(&sink));
}
