use super::*;
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
