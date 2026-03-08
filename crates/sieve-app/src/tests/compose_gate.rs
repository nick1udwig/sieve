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
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        trusted_effects: Vec::new(),
        extracted_evidence: Vec::new(),
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

#[test]
fn compose_gate_followup_signal_ignores_interstitial_continue_when_explicit_answer_exists() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "what is the top video?".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        trusted_effects: Vec::new(),
        extracted_evidence: vec![sieve_llm::ResponseEvidenceRecord {
            ref_id: "artifact-1".to_string(),
            summary: "The first visible video result is Jordan Peterson Live on Tour.".to_string(),
            page_state: Some("result_list".to_string()),
            blockers: vec![],
            source_urls: vec!["https://www.youtube.com/watch?v=yuc807SP_gA".to_string()],
            items: vec![],
            answer_candidate: Some(sieve_llm::ResponseAnswerCandidate {
                target: "top_video".to_string(),
                item_kind: "video".to_string(),
                title: "Jordan Peterson Live on Tour: The Hidden Key to a Fulfilling Life"
                    .to_string(),
                url: Some("https://www.youtube.com/watch?v=yuc807SP_gA".to_string()),
                support: "explicit_item".to_string(),
                rank: Some(1),
            }),
        }],
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("agent-browser snapshot --session ytsearch".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 28058,
                line_count: 398,
            }],
        }],
    };
    let gate = Some(ComposeGateOutput {
        verdict: "REVISE".to_string(),
        reason: Some("evidence hit login/interstitial pages".to_string()),
        continue_code: Some(115),
    });

    assert!(compose_gate_followup_signal(gate.as_ref(), &input).is_none());
}

#[test]
fn compose_gate_ignores_gate_negation_when_trusted_effect_exists() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "in one minute send me a message saying hi".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: Some("scheduled reminder".to_string()),
        trusted_effects: vec![sieve_types::TrustedToolEffect::CronAdded {
            job_id: "cron-1".to_string(),
            target: sieve_types::AutomationTarget::Main,
            run_at_ms: 1_762_839_600_000,
            prompt: "hi".to_string(),
            delivery_mode: sieve_types::AutomationDeliveryMode::MainSessionMessage,
        }],
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![],
    };
    let gate = ComposeGateOutput {
        verdict: "REVISE".to_string(),
        reason: Some("unsupported external action; cannot send that message".to_string()),
        continue_code: Some(102),
    };

    assert!(compose_gate_requires_retry(
        "Scheduled. I'll send `hi` here in about a minute.",
        "in one minute send me a message saying hi",
        &input,
        Some(&gate)
    )
    .is_none());
    assert!(compose_gate_followup_signal(Some(&gate), &input).is_none());
}

#[test]
fn compose_gate_retries_when_message_negates_trusted_effect() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "in one minute send me a message saying hi".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: Some("scheduled reminder".to_string()),
        trusted_effects: vec![sieve_types::TrustedToolEffect::CronAdded {
            job_id: "cron-1".to_string(),
            target: sieve_types::AutomationTarget::Main,
            run_at_ms: 1_762_839_600_000,
            prompt: "hi".to_string(),
            delivery_mode: sieve_types::AutomationDeliveryMode::MainSessionMessage,
        }],
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![],
    };

    assert!(compose_gate_requires_retry(
        "I can't actually send you a message saying hi.",
        "in one minute send me a message saying hi",
        &input,
        None,
    )
    .is_some());
}
