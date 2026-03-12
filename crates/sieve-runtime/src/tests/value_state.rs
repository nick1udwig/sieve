use super::*;
use sieve_types::{SinkChannel, SinkPermission, Source};

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
                        channel: SinkChannel::Body,
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
    assert!(!first_transition.release_value_existed);
    assert_ne!(
        first_transition.release_value_ref,
        ValueRef("v456".to_string())
    );

    let second_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-2".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v456".to_string()),
                        sink: SinkKey(sink.to_string()),
                        channel: SinkChannel::Body,
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
    assert!(second_transition.release_value_existed);
    assert_eq!(
        second_transition.release_value_ref,
        first_transition.release_value_ref
    );

    let source_label = runtime
        .value_label(&ValueRef("v456".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert!(!source_label.allowed_sinks.contains(&SinkPermission {
        sink: SinkKey(sink.to_string()),
        channel: SinkChannel::Body,
    }));
    let release_label = runtime
        .value_label(&first_transition.release_value_ref)
        .expect("read release label")
        .expect("release label present");
    assert!(release_label.allowed_sinks.contains(&SinkPermission {
        sink: SinkKey(sink.to_string()),
        channel: SinkChannel::Body,
    }));
}

#[tokio::test]
async fn declassify_value_once_is_channel_scoped() {
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
            ValueRef("v789".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed value state");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-channel-scope".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v789".to_string()),
                        sink: SinkKey(sink.to_string()),
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
            run_id: RunId("run-channel-scope".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2002,
        })
        .expect("resolve approval");
    runtime_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");

    let label = runtime
        .value_label(&ValueRef("v789".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert!(!label.allowed_sinks.contains(&SinkPermission {
        sink: SinkKey(sink.to_string()),
        channel: SinkChannel::Body,
    }));
    assert!(!label.allowed_sinks.contains(&SinkPermission {
        sink: SinkKey(sink.to_string()),
        channel: SinkChannel::Header,
    }));
}

#[tokio::test]
async fn endorse_value_once_trusted_string_is_denied_without_approval() {
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
            ValueRef("v-string".to_string()),
            ValueLabel {
                integrity: Integrity::Untrusted,
                provenance: BTreeSet::new(),
                allowed_sinks: BTreeSet::new(),
                capacity_type: sieve_types::CapacityType::TrustedString,
            },
        )
        .expect("seed trusted_string value");

    let transition = runtime
        .endorse_value_once(
            RunId("run-string-endorse".to_string()),
            EndorseRequest {
                value_ref: ValueRef("v-string".to_string()),
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
        .value_label(&ValueRef("v-string".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert_eq!(label.integrity, Integrity::Untrusted);
}

#[tokio::test]
async fn declassify_value_once_trusted_string_is_denied_without_approval() {
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
    let sink = SinkKey("https://api.example.com/v1/upload".to_string());
    runtime
        .upsert_value_label(
            ValueRef("v-string".to_string()),
            ValueLabel {
                integrity: Integrity::Trusted,
                provenance: BTreeSet::new(),
                allowed_sinks: BTreeSet::new(),
                capacity_type: sieve_types::CapacityType::TrustedString,
            },
        )
        .expect("seed trusted_string value");

    let transition = runtime
        .declassify_value_once(
            RunId("run-string-declassify".to_string()),
            DeclassifyRequest {
                value_ref: ValueRef("v-string".to_string()),
                sink: sink.clone(),
                channel: SinkChannel::Body,
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
        .value_label(&ValueRef("v-string".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert!(!label.allowed_sinks.contains(&SinkPermission {
        sink,
        channel: SinkChannel::Body,
    }));
}

#[tokio::test]
async fn declassify_value_once_mints_release_value_with_bounded_sink_scope() {
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
            ValueRef("v900".to_string()),
            label_with_sinks(Integrity::Trusted, &[]),
        )
        .expect("seed source value");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .declassify_value_once(
                    RunId("run-derived-release".to_string()),
                    DeclassifyRequest {
                        value_ref: ValueRef("v900".to_string()),
                        sink: SinkKey(sink.to_string()),
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
            run_id: RunId("run-derived-release".to_string()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 2003,
        })
        .expect("resolve approval");
    let transition = runtime_task
        .await
        .expect("task join")
        .expect("runtime ok")
        .expect("approved transition");

    let source_label = runtime
        .value_label(&ValueRef("v900".to_string()))
        .expect("read source label")
        .expect("source label present");
    assert!(source_label.allowed_sinks.is_empty());

    let release_label = runtime
        .value_label(&transition.release_value_ref)
        .expect("read release label")
        .expect("release label present");
    assert_eq!(release_label.integrity, Integrity::Trusted);
    assert_eq!(release_label.capacity_type, sieve_types::CapacityType::Enum);
    assert_eq!(release_label.allowed_sinks.len(), 1);
    assert!(release_label.allowed_sinks.contains(&SinkPermission {
        sink: SinkKey(sink.to_string()),
        channel: SinkChannel::Body,
    }));
    assert!(release_label.provenance.iter().any(|source| {
        matches!(
            source,
            Source::Tool {
                tool_name,
                inner_sources
            } if tool_name == "declassify"
                && inner_sources.contains("v900")
        )
    }));
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
                channel: SinkChannel::Body,
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
    assert!(!label.allowed_sinks.contains(&SinkPermission {
        sink,
        channel: SinkChannel::Body,
    }));
}
