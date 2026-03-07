use super::*;
use crate::turn::{build_response_evidence_records, response_evidence_fingerprint};

#[tokio::test]
async fn render_assistant_message_replaces_known_tokens() {
    let message = "trace path: [[ref:trace:run-1]]";
    let refs = BTreeMap::from([(
        "trace:run-1".to_string(),
        RenderRef::Literal {
            value: "/tmp/sieve/trace/run-1".to_string(),
        },
    )]);
    let referenced_ref_ids = BTreeSet::from(["trace:run-1".to_string()]);
    let summarized_ref_ids = BTreeSet::new();

    let expanded = render_assistant_message(
        message,
        &referenced_ref_ids,
        &summarized_ref_ids,
        &refs,
        &EchoSummaryModel,
        &RunId("run-test".to_string()),
    )
    .await;
    assert_eq!(expanded, "trace path: /tmp/sieve/trace/run-1");
}

#[tokio::test]
async fn render_assistant_message_does_not_summarize_unreferenced_selected_refs() {
    let message = "done";
    let artifact_path = std::env::temp_dir().join("render-no-token-artifact.txt");
    std::fs::write(&artifact_path, "hello from artifact").expect("write artifact");
    let refs = BTreeMap::from([(
        "artifact-1".to_string(),
        RenderRef::Artifact {
            path: artifact_path,
            byte_count: 19,
            line_count: 1,
        },
    )]);
    let summary_model = QueuedSummaryModel::new(vec![Ok("should not be used".to_string())]);

    let expanded = render_assistant_message(
        message,
        &BTreeSet::new(),
        &BTreeSet::from(["artifact-1".to_string()]),
        &refs,
        &summary_model,
        &RunId("run-test".to_string()),
    )
    .await;

    assert_eq!(expanded, "done");
    assert_eq!(summary_model.call_count(), 0);
}

#[test]
fn build_response_turn_input_handles_zero_tool_turn() {
    let run_id = RunId("run-1".to_string());
    let planner_result = PlannerRunResult {
        thoughts: Some("chat reply".to_string()),
        tool_results: Vec::new(),
    };

    let (input, refs) = build_response_turn_input(
        &run_id,
        "hi",
        test_delivery_context(
            sieve_types::DeliveryChannel::Stdin,
            InteractionModality::Text,
        ),
        test_resolved_personality(),
        &planner_result,
    );
    assert_eq!(input.run_id, run_id);
    assert_eq!(input.trusted_user_message, "hi");
    assert_eq!(input.response_modality, InteractionModality::Text);
    assert_eq!(input.planner_thoughts.as_deref(), Some("chat reply"));
    assert!(input.tool_outcomes.is_empty());
    assert!(refs.is_empty());
}

#[test]
fn requires_output_visibility_detects_non_empty_stdout_or_stderr_refs() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "show output".to_string(),
        delivery_context: test_delivery_context(
            sieve_types::DeliveryChannel::Stdin,
            InteractionModality::Text,
        ),
        response_modality: InteractionModality::Text,
        resolved_personality: test_resolved_personality(),
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("pwd".to_string()),
            failure_reason: None,
            refs: vec![
                ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 42,
                    line_count: 2,
                },
                ResponseRefMetadata {
                    ref_id: "artifact-2".to_string(),
                    kind: "stderr".to_string(),
                    byte_count: 0,
                    line_count: 0,
                },
            ],
        }],
    };

    assert!(requires_output_visibility(&input));
}

#[test]
fn requires_output_visibility_skips_when_user_did_not_ask_for_output() {
    let input = ResponseTurnInput {
            run_id: RunId("run-1".to_string()),
            trusted_user_message: "What is the weather tomorrow in Livermore?".to_string(),
            delivery_context: test_delivery_context(
                sieve_types::DeliveryChannel::Stdin,
                InteractionModality::Text,
            ),
            response_modality: InteractionModality::Text,
            resolved_personality: test_resolved_personality(),
            planner_thoughts: None,
            extracted_evidence: Vec::new(),
            tool_outcomes: vec![ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "executed".to_string(),
                attempted_command: Some(
                    "bravesearch search --query \"Livermore CA weather tomorrow\" --count 5 --output json"
                        .to_string(),
                ),
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: "artifact-1".to_string(),
                    kind: "stdout".to_string(),
                    byte_count: 1024,
                    line_count: 12,
                }],
            }],
        };

    assert!(!requires_output_visibility(&input));
}

#[test]
fn response_has_visible_selected_output_requires_message_token() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "show output".to_string(),
        delivery_context: test_delivery_context(
            sieve_types::DeliveryChannel::Stdin,
            InteractionModality::Text,
        ),
        response_modality: InteractionModality::Text,
        resolved_personality: test_resolved_personality(),
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("pwd".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 4,
                line_count: 1,
            }],
        }],
    };

    let no_token = sieve_llm::ResponseTurnOutput {
        message: "completed".to_string(),
        referenced_ref_ids: BTreeSet::from(["artifact-1".to_string()]),
        summarized_ref_ids: BTreeSet::new(),
    };
    assert!(!response_has_visible_selected_output(&input, &no_token));

    let with_token = sieve_llm::ResponseTurnOutput {
        message: "output: [[ref:artifact-1]]".to_string(),
        referenced_ref_ids: BTreeSet::from(["artifact-1".to_string()]),
        summarized_ref_ids: BTreeSet::new(),
    };
    assert!(response_has_visible_selected_output(&input, &with_token));
}

#[test]
fn response_has_visible_selected_output_accepts_summary_token() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "summarize output".to_string(),
        delivery_context: test_delivery_context(
            sieve_types::DeliveryChannel::Stdin,
            InteractionModality::Text,
        ),
        response_modality: InteractionModality::Text,
        resolved_personality: test_resolved_personality(),
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("pwd".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-2".to_string(),
                kind: "stderr".to_string(),
                byte_count: 10,
                line_count: 2,
            }],
        }],
    };

    let response = sieve_llm::ResponseTurnOutput {
        message: "summary: [[summary:artifact-2]]".to_string(),
        referenced_ref_ids: BTreeSet::new(),
        summarized_ref_ids: BTreeSet::from(["artifact-2".to_string()]),
    };
    assert!(response_has_visible_selected_output(&input, &response));
}

#[test]
fn response_evidence_fingerprint_ignores_ref_ids_for_identical_evidence() {
    let make_input = |ref_id: &str| ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "what is the top video?".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: vec![sieve_llm::ResponseEvidenceRecord {
            ref_id: ref_id.to_string(),
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
            outcome: "executed mainline (exit_code=Some(0))".to_string(),
            attempted_command: Some("agent-browser snapshot --session ytsearch".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: ref_id.to_string(),
                kind: "stdout".to_string(),
                byte_count: 28058,
                line_count: 398,
            }],
        }],
    };

    assert_eq!(
        response_evidence_fingerprint(&make_input("artifact-1")),
        response_evidence_fingerprint(&make_input("artifact-2"))
    );
}

#[tokio::test]
async fn build_response_evidence_records_batches_and_parses_structured_output() {
    let input = ResponseTurnInput {
        run_id: RunId("run-1".to_string()),
        trusted_user_message: "what is the top video?".to_string(),
        response_modality: InteractionModality::Text,
        planner_thoughts: None,
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "executed".to_string(),
            attempted_command: Some("agent-browser snapshot --session ytsearch".to_string()),
            failure_reason: None,
            refs: vec![ResponseRefMetadata {
                ref_id: "artifact-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 128,
                line_count: 8,
            }],
        }],
    };
    let artifact_path = std::env::temp_dir().join("response-evidence-artifact.txt");
    std::fs::write(&artifact_path, "browser snapshot text").expect("write artifact");
    let render_refs = BTreeMap::from([(
        "artifact-1".to_string(),
        RenderRef::Artifact {
            path: artifact_path,
            byte_count: 128,
            line_count: 8,
        },
    )]);
    let summary_model = QueuedSummaryModel::new(vec![Ok(
        "{\"records\":[{\"ref_id\":\"artifact-1\",\"summary\":\"The first video result is Jordan Peterson Live on Tour: The Hidden Key to a Fulfilling Life.\",\"page_state\":\"result_list\",\"blockers\":[],\"source_urls\":[\"https://www.youtube.com/watch?v=yuc807SP_gA\"],\"items\":[{\"kind\":\"video\",\"rank\":1,\"title\":\"Jordan Peterson Live on Tour: The Hidden Key to a Fulfilling Life\",\"url\":\"https://www.youtube.com/watch?v=yuc807SP_gA\"}],\"answer_candidate\":{\"target\":\"top_video\",\"item_kind\":\"video\",\"title\":\"Jordan Peterson Live on Tour: The Hidden Key to a Fulfilling Life\",\"url\":\"https://www.youtube.com/watch?v=yuc807SP_gA\",\"support\":\"explicit_item\",\"rank\":1}}]}".to_string(),
    )]);
    let mut summary_calls = 0usize;

    let records = build_response_evidence_records(
        &summary_model,
        &RunId("run-1".to_string()),
        "what is the top video?",
        &input,
        &render_refs,
        &mut summary_calls,
        4,
    )
    .await;

    assert_eq!(summary_calls, 1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].page_state.as_deref(), Some("result_list"));
    assert_eq!(
        records[0]
            .answer_candidate
            .as_ref()
            .map(|candidate| candidate.title.as_str()),
        Some("Jordan Peterson Live on Tour: The Hidden Key to a Fulfilling Life")
    );
}
