use super::*;
#[test]
fn compose_quality_retry_treats_verbose_pass_as_pass() {
    let composed = "Here is a direct answer.";
    let gate = Some("Quality gate verdict: PASS because the answer is direct.");
    assert!(compose_quality_requires_retry(composed, gate).is_none());
}

#[test]
fn gate_requires_retry_treats_pass_as_no_retry() {
    assert!(gate_requires_retry(Some("PASS")).is_none());
    assert!(gate_requires_retry(Some("verdict: pass")).is_none());
    assert!(gate_requires_retry(Some("REVISE: unsupported claim")).is_some());
    assert!(gate_requires_retry(Some("This response lacks specific weather details.")).is_some());
}

#[test]
fn combine_gate_reasons_joins_non_empty_reasons() {
    let combined = combine_gate_reasons(&[
        Some("REVISE: quality".to_string()),
        None,
        Some("REVISE: grounding".to_string()),
    ]);
    assert_eq!(
        combined.as_deref(),
        Some("REVISE: quality | REVISE: grounding")
    );
}

#[test]
fn denied_outcomes_only_message_reports_attempt_and_reason() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "weather".to_string(),
        delivery_context: test_delivery_context(
            sieve_types::DeliveryChannel::Stdin,
            InteractionModality::Text,
        ),
        response_modality: InteractionModality::Text,
        resolved_personality: test_resolved_personality(),
        planner_thoughts: None,
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "denied".to_string(),
            attempted_command: Some(
                "bravesearch search \"Livermore CA weather tomorrow\" --count 5 --format json"
                    .to_string(),
            ),
            failure_reason: Some("unknown command denied by mode".to_string()),
            refs: vec![],
        }],
    };

    let message = denied_outcomes_only_message(&input).expect("must generate denied message");
    assert!(message.contains("I tried `bravesearch search"));
    assert!(message.contains("unknown command denied by mode"));
    assert!(message.contains("different command path"));
}

#[test]
fn obvious_meta_compose_pattern_catches_user_asks_diagnostic_format() {
    let message = "User asks: “What is the weather?” Diagnostic notes the draft is weak.";
    assert!(obvious_meta_compose_pattern(message));
}

#[test]
fn obvious_meta_compose_pattern_catches_evidence_summary_diagnostic_format() {
    let message = "The evidence summary explicitly says no relevant evidence was found.";
    assert!(obvious_meta_compose_pattern(message));
}

#[test]
fn strip_unexpanded_render_tokens_removes_ref_markers() {
    let message = "answer [[ref:artifact-1]] and [[summary:artifact-2]] done";
    assert_eq!(strip_unexpanded_render_tokens(message), "answer  and  done");
}
