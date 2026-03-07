use super::*;
#[test]
fn compose_quality_followup_only_triggers_for_missing_evidence() {
    let with_refs = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "weather".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some(
                "bravesearch search --query \"weather\" --count 5 --output json".to_string(),
            ),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 64,
                line_count: 1,
            }],
        }],
    };
    let signal = compose_quality_followup_signal(
        Some("REVISE: doesn't directly answer and is missing evidence."),
        &with_refs,
    );
    assert_eq!(signal, Some(PlannerGuidanceSignal::ContinueRefineApproach));

    let generic_signal = compose_quality_followup_signal(
        Some("The response lacks specific weather details."),
        &with_refs,
    );
    assert_eq!(
        generic_signal,
        Some(PlannerGuidanceSignal::ContinueRefineApproach)
    );

    let style_signal =
        compose_quality_followup_signal(Some("REVISE: third-person meta narration."), &with_refs);
    assert!(style_signal.is_none());
}

#[test]
fn compose_quality_followup_maps_required_parameter_signal() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "where do i live".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "lcm_expand_query".to_string(),
            outcome: "executed".to_string(),
            attempted_command: None,
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 32,
                line_count: 1,
            }],
        }],
    };

    let signal = compose_quality_followup_signal(
        Some("REVISE: missing required parameter; please specify."),
        &input,
    );
    assert_eq!(
        signal,
        Some(PlannerGuidanceSignal::ContinueNeedRequiredParameter)
    );
}

#[test]
fn compose_quality_followup_maps_denied_tool_signal() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "weather tomorrow".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "denied".to_string(),
            attempted_command: Some("curl -s 'https://wttr.in'".to_string()),
            failure_reason: Some("unknown command denied by mode".to_string()),
            refs: vec![],
        }],
    };

    let signal = compose_quality_followup_signal(Some("REVISE: tool call was denied."), &input);
    assert_eq!(
        signal,
        Some(PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool)
    );
}

#[test]
fn compose_quality_followup_maps_conflict_signal() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "compare claims".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("some command".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 128,
                line_count: 4,
            }],
        }],
    };
    let signal = compose_quality_followup_signal(
        Some("REVISE: sources conflict and are inconsistent."),
        &input,
    );
    assert_eq!(
        signal,
        Some(PlannerGuidanceSignal::ContinueResolveSourceConflict)
    );
}

#[test]
fn compose_quality_followup_maps_primary_content_fetch_signal() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "latest status".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some(
                "bravesearch search --query \"status\" --count 5 --output json".to_string(),
            ),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 64,
                line_count: 1,
            }],
        }],
    };
    let signal = compose_quality_followup_signal(
        Some("REVISE: discovery/search snippets only; missing primary content."),
        &input,
    );
    assert_eq!(
        signal,
        Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch)
    );
}

#[test]
fn compose_quality_followup_maps_url_extraction_signal() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "summarize".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("curl -sS https://example.com".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 64,
                line_count: 1,
            }],
        }],
    };
    let signal = compose_quality_followup_signal(
        Some("REVISE: need URL extraction from fetched content before next step."),
        &input,
    );
    assert_eq!(
        signal,
        Some(PlannerGuidanceSignal::ContinueNeedUrlExtraction)
    );
}

#[test]
fn compose_quality_followup_suppresses_continue_when_explicit_answer_candidate_exists() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "what is the top video?".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: vec![sieve_llm::ResponseEvidenceRecord {
            ref_id: "artifact-1".to_string(),
            summary: "The first video result is Jordan Peterson Live on Tour.".to_string(),
            page_state: Some("result_list".to_string()),
            blockers: vec![],
            source_urls: vec!["https://www.youtube.com/watch?v=yuc807SP_gA".to_string()],
            items: vec![sieve_llm::ResponseEvidenceItem {
                kind: "video".to_string(),
                rank: Some(1),
                title: "Jordan Peterson Live on Tour: The Hidden Key to a Fulfilling Life"
                    .to_string(),
                url: Some("https://www.youtube.com/watch?v=yuc807SP_gA".to_string()),
            }],
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

    let signal = compose_quality_followup_signal(
        Some("REVISE: evidence shows login/interstitial pages and needs higher quality confirmation."),
        &input,
    );
    assert!(signal.is_none());
}
