use super::*;
#[tokio::test]
async fn e2e_fake_compose_continue_stops_when_no_new_evidence() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: Some("step-1".to_string()),
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: Some("step-2".to_string()),
            tool_calls: Vec::new(),
        }),
    ]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
    ]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![
        Ok(sieve_llm::ResponseTurnOutput {
            message: "Draft one".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        }),
        Ok(sieve_llm::ResponseTurnOutput {
            message: "Draft two".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        }),
    ]));
    let summary_impl = Arc::new(QueuedSummaryModel::new(vec![
        Ok("Cycle 1 response.".to_string()),
        Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":102}".to_string()),
        Ok("Cycle 2 response.".to_string()),
        Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":102}".to_string()),
    ]));
    let summary: Arc<dyn SummaryModel> = summary_impl.clone();
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner: planner.clone(),
            guidance,
            response,
            summary,
        },
        vec!["bash".to_string()],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn("Hi I live in Livermore ca")
        .await
        .expect("compose no-new-evidence guard turn should succeed");

    assert_eq!(
        planner.call_count(),
        2,
        "compose follow-up should run once, then stop on repeated evidence"
    );
    assert_eq!(
        summary_impl.call_count(),
        4,
        "compose pass should be bounded to two cycles (compose+gate each)"
    );
    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(
        assistant.last().map(String::as_str),
        Some("Cycle 2 response."),
        "second compose cycle response should be final output"
    );
}

#[tokio::test]
async fn e2e_fake_compose_summary_budget_caps_calls() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some("step-1".to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "Draft one".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary_impl = Arc::new(QueuedSummaryModel::new(vec![
        Ok("Budgeted response.".to_string()),
        Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":102}".to_string()),
    ]));
    let summary: Arc<dyn SummaryModel> = summary_impl.clone();
    let mut harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner: planner.clone(),
            guidance,
            response,
            summary,
        },
        vec!["bash".to_string()],
        E2E_POLICY_BASE,
    );
    harness.cfg.max_summary_calls_per_turn = 2;

    harness
        .run_text_turn("Hi I live in Livermore ca")
        .await
        .expect("compose summary budget turn should succeed");

    assert_eq!(
        planner.call_count(),
        1,
        "summary budget should block additional compose follow-up cycles"
    );
    assert_eq!(
        summary_impl.call_count(),
        2,
        "summary calls should stop at configured per-turn budget"
    );
    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(
        assistant.last().map(String::as_str),
        Some("Budgeted response."),
        "budgeted compose response should still render a final assistant reply"
    );
}

#[tokio::test]
async fn e2e_fake_general_compose_pass_rewrites_final_message() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("direct response".to_string()),
        tool_calls: Vec::new(),
    };
    let response_output = sieve_llm::ResponseTurnOutput {
        message: "Draft response that is too wordy.".to_string(),
        referenced_ref_ids: BTreeSet::new(),
        summarized_ref_ids: BTreeSet::new(),
    };
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(planner_output)]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> =
        Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
    let summary: Arc<dyn SummaryModel> = Arc::new(QueuedSummaryModel::new(vec![
        Ok("Hello there.".to_string()),
        Ok("PASS".to_string()),
    ]));
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn("Please greet me briefly.")
        .await
        .expect("compose pass turn should succeed");

    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(assistant, vec!["Hello there.".to_string()]);
}

#[tokio::test]
async fn e2e_fake_compose_retries_on_meta_narration() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some("chat".to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response_output = sieve_llm::ResponseTurnOutput {
        message: "Hey!".to_string(),
        referenced_ref_ids: BTreeSet::new(),
        summarized_ref_ids: BTreeSet::new(),
    };
    let response: Arc<dyn ResponseModel> =
        Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
    let summary: Arc<dyn SummaryModel> = Arc::new(QueuedSummaryModel::new(vec![
        Ok("The assistant is ready to help and asks how it can assist.".to_string()),
        Ok("PASS".to_string()),
        Ok("I'm doing well, thanks for asking. How can I help?".to_string()),
    ]));
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn("Reply directly to this greeting: Hi how are you?")
        .await
        .expect("meta compose retry turn should succeed");

    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(assistant.len(), 1);
    assert!(
        assistant[0].starts_with("I'm doing well"),
        "compose retry should replace third-person meta narration"
    );
}

#[tokio::test]
async fn e2e_fake_compose_falls_back_to_draft_on_evidence_summary_diagnostic_leak() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some("chat".to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let draft = "Thanks for sharing that you live in Livermore, CA. What can I help with today?"
        .to_string();
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: draft.clone(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(QueuedSummaryModel::new(vec![
            Ok("The evidence summary explicitly says no relevant evidence was found, so stating the user’s location as a verified fact would be ungrounded.".to_string()),
            Ok("PASS".to_string()),
            Ok("The evidence summary explicitly says no relevant evidence was found.".to_string()),
            Ok("PASS".to_string()),
        ]));
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn("Hi I live in Livermore ca")
        .await
        .expect("diagnostic leak turn should succeed");

    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(assistant, vec![draft]);
}

#[tokio::test]
async fn e2e_fake_planner_error_emits_assistant_error_for_user_visibility() {
    let planner: Arc<dyn PlannerModel> = Arc::new(QueuedPlannerModel::new(vec![Err(
        LlmError::Backend("planner boom".to_string()),
    )]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(Vec::new()));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["bash".to_string()],
        E2E_POLICY_BASE,
    );

    let err = harness
        .run_text_turn("Use bash to run exactly: pwd")
        .await
        .expect_err("planner failure must propagate to caller");
    assert!(err.contains("planner model failed"));

    let events = harness.runtime_events();
    let assistant = assistant_messages(&events);
    assert_eq!(assistant.len(), 1);
    assert!(
        assistant[0].starts_with("error:"),
        "assistant-visible fallback must be emitted on planner failure"
    );

    let records = harness.jsonl_records();
    let assistant_errors = assistant_errors_from_conversation(&records);
    assert_eq!(assistant_errors.len(), 1);
    assert!(assistant_errors[0].contains("planner model failed"));
}
