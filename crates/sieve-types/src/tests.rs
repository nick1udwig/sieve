#![cfg(test)]

use crate::*;
use serde_json::Value;

fn assert_matches_schema(schema_json: &str, instance: &Value) {
    let schema: Value = serde_json::from_str(schema_json).expect("parse schema");
    let validator = jsonschema::validator_for(&schema).expect("compile schema");
    validator
        .validate(instance)
        .expect("instance should satisfy schema");
}

#[test]
fn approval_requested_event_json_round_trip() {
    let event = ApprovalRequestedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("apr_1".into()),
        run_id: RunId("run_1".into()),
        command_segments: vec![
            CommandSegment {
                argv: vec!["echo".into(), "hello".into()],
                operator_before: None,
            },
            CommandSegment {
                argv: vec!["wc".into(), "-c".into()],
                operator_before: Some(CompositionOperator::Pipe),
            },
        ],
        inferred_capabilities: vec![Capability {
            resource: Resource::Proc,
            action: Action::Exec,
            scope: "/usr/bin/wc".into(),
        }],
        blocked_rule_id: "rule.command.rm_rf".into(),
        reason: "deny_with_approval".into(),
        created_at_ms: 1_717_171_717_000,
    };

    let encoded = serde_json::to_string(&event).expect("serialize");
    let decoded: ApprovalRequestedEvent = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, event);
}

#[test]
fn approval_resolved_event_json_round_trip() {
    let event = ApprovalResolvedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("apr_2".into()),
        run_id: RunId("run_2".into()),
        action: ApprovalAction::ApproveOnce,
        created_at_ms: 1_717_171_718_000,
    };

    let encoded = serde_json::to_string(&event).expect("serialize");
    let decoded: ApprovalResolvedEvent = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, event);
}

#[test]
fn approval_requested_event_matches_schema() {
    let event = ApprovalRequestedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("apr_schema_1".into()),
        run_id: RunId("run_schema_1".into()),
        command_segments: vec![CommandSegment {
            argv: vec!["rm".into(), "-rf".into(), "/tmp/demo".into()],
            operator_before: None,
        }],
        inferred_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp/demo".into(),
        }],
        blocked_rule_id: "rule.command.rm_rf".into(),
        reason: "requires approval".into(),
        created_at_ms: 1_717_171_800_000,
    };
    let instance = serde_json::to_value(event).expect("serialize event");
    let schema = include_str!("../../../schemas/approval-requested-event.schema.json");
    assert_matches_schema(schema, &instance);
}

#[test]
fn approval_resolved_event_matches_schema() {
    let event = ApprovalResolvedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("apr_schema_2".into()),
        run_id: RunId("run_schema_2".into()),
        action: ApprovalAction::ApproveOnce,
        created_at_ms: 1_717_171_801_000,
    };
    let instance = serde_json::to_value(event).expect("serialize event");
    let schema = include_str!("../../../schemas/approval-resolved-event.schema.json");
    assert_matches_schema(schema, &instance);
}

#[test]
fn runtime_event_json_round_trip() {
    let event = RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
        schema_version: 1,
        run_id: RunId("run_3".into()),
        decision: PolicyDecision {
            kind: PolicyDecisionKind::DenyWithApproval,
            reason: "mutating command from untrusted context".into(),
            blocked_rule_id: Some("policy.integrity.001".into()),
        },
        inferred_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp/out.txt".into(),
        }],
        trace_path: Some("/home/user/.sieve/logs/traces/run_3".into()),
        created_at_ms: 1_717_171_719_000,
    });

    let encoded = serde_json::to_string(&event).expect("serialize");
    let decoded: RuntimeEvent = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, event);
}

#[test]
fn assistant_message_event_json_round_trip() {
    let event = AssistantMessageEvent {
        schema_version: 1,
        run_id: RunId("run_4".into()),
        message: "all done".to_string(),
        created_at_ms: 1_717_171_720_000,
    };

    let encoded = serde_json::to_string(&event).expect("serialize");
    let decoded: AssistantMessageEvent = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, event);
}

#[test]
fn delivery_context_json_round_trip() {
    let context = DeliveryContext {
        channel: DeliveryChannel::Telegram,
        destination: Some("42".to_string()),
        input_modality: InteractionModality::Text,
        response_modality: InteractionModality::Audio,
    };

    let encoded = serde_json::to_string(&context).expect("serialize");
    let decoded: DeliveryContext = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, context);
}

#[test]
fn resolved_personality_json_round_trip() {
    let personality = ResolvedPersonality {
        identity: "helpful assistant".to_string(),
        style: "clear and concise".to_string(),
        emoji_policy: EmojiPolicy::Light,
        verbosity: ResponseVerbosity::Telegraph,
        channel_guidance: vec!["Keep replies short for chat.".to_string()],
        custom_instructions: vec!["Skip filler.".to_string()],
    };

    let encoded = serde_json::to_string(&personality).expect("serialize");
    let decoded: ResolvedPersonality = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, personality);
}

#[test]
fn endorse_payload_json_round_trip() {
    let request = EndorseRequest {
        value_ref: ValueRef("v123".into()),
        target_integrity: Integrity::Trusted,
        reason: Some("user approved control use".into()),
    };
    let response = EndorseResponse {
        value_ref: ValueRef("v123e".into()),
        integrity: Integrity::Trusted,
    };

    let request_encoded = serde_json::to_string(&request).expect("serialize");
    let response_encoded = serde_json::to_string(&response).expect("serialize");
    let request_decoded: EndorseRequest =
        serde_json::from_str(&request_encoded).expect("deserialize");
    let response_decoded: EndorseResponse =
        serde_json::from_str(&response_encoded).expect("deserialize");

    assert_eq!(request_decoded, request);
    assert_eq!(response_decoded, response);
}

#[test]
fn declassify_payload_json_round_trip() {
    let request = DeclassifyRequest {
        value_ref: ValueRef("v456".into()),
        sink: SinkKey("https://api.example.com/v1/upload".into()),
        reason: Some("user approved outbound upload".into()),
    };
    let response = DeclassifyResponse {
        value_ref: ValueRef("v456d".into()),
        allowed_sinks_added: vec![SinkKey("https://api.example.com/v1/upload".into())],
    };

    let request_encoded = serde_json::to_string(&request).expect("serialize");
    let response_encoded = serde_json::to_string(&response).expect("serialize");
    let request_decoded: DeclassifyRequest =
        serde_json::from_str(&request_encoded).expect("deserialize");
    let response_decoded: DeclassifyResponse =
        serde_json::from_str(&response_encoded).expect("deserialize");

    assert_eq!(request_decoded, request);
    assert_eq!(response_decoded, response);
}

#[test]
fn planner_guidance_signal_new_codes_round_trip() {
    let cases = vec![
        (104u16, PlannerGuidanceSignal::ContinueNeedRequiredParameter),
        (
            105u16,
            PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence,
        ),
        (
            106u16,
            PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint,
        ),
        (
            107u16,
            PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool,
        ),
        (
            108u16,
            PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
        ),
        (109u16, PlannerGuidanceSignal::ContinueResolveSourceConflict),
        (
            110u16,
            PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch,
        ),
        (111u16, PlannerGuidanceSignal::ContinueNeedUrlExtraction),
        (
            112u16,
            PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl,
        ),
        (
            113u16,
            PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction,
        ),
        (
            114u16,
            PlannerGuidanceSignal::ContinueNeedCurrentPageInspection,
        ),
        (
            115u16,
            PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial,
        ),
        (
            116u16,
            PlannerGuidanceSignal::ContinueNeedCommandReformulation,
        ),
        (203u16, PlannerGuidanceSignal::FinalSingleFactReady),
        (
            204u16,
            PlannerGuidanceSignal::FinalConflictingFactsWithRange,
        ),
        (205u16, PlannerGuidanceSignal::FinalNoToolActionNeeded),
        (
            302u16,
            PlannerGuidanceSignal::StopNoAllowedToolCanSatisfyTask,
        ),
    ];

    for (code, expected) in cases {
        let signal = PlannerGuidanceSignal::try_from(code).expect("must parse new signal");
        assert_eq!(signal, expected);
        assert_eq!(signal.code(), code);
    }
}

#[test]
fn modality_contract_json_round_trip() {
    let contract = ModalityContract {
        input: InteractionModality::Audio,
        response: InteractionModality::Text,
        override_reason: Some(ModalityOverrideReason::ToolFailure),
    };

    let encoded = serde_json::to_string(&contract).expect("serialize");
    let decoded: ModalityContract = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, contract);
}
