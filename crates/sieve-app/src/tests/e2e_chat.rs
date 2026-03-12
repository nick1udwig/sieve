use super::*;
#[tokio::test]
async fn e2e_fake_greeting_uses_guided_zero_tool_turn_without_approval() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("chat only".to_string()),
        tool_calls: Vec::new(),
    };
    let response_output = sieve_llm::ResponseTurnOutput {
        message: "Yes, I can hear you.".to_string(),
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
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
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
        .run_text_turn("Hi can you hear me?")
        .await
        .expect("greeting turn should succeed");

    let events = harness.runtime_events();
    assert_eq!(count_approval_requested(&events), 0);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
        "greeting should not trigger tool policy checks"
    );
    let assistant = assistant_messages(&events);
    assert_eq!(assistant, vec!["Yes, I can hear you.".to_string()]);

    let records = harness.jsonl_records();
    let conversation = conversation_messages(&records);
    assert_eq!(conversation.len(), 2);
    assert_eq!(conversation[0].0, "user");
    assert_eq!(conversation[1].0, "assistant");
    assert_eq!(conversation[1].1, "Yes, I can hear you.");
    assert!(
        assistant_errors_from_conversation(&records).is_empty(),
        "greeting flow should not emit assistant error conversation"
    );
}

#[tokio::test]
async fn e2e_fake_zero_tool_final_no_tool_action_needed_skips_compose() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("chat only".to_string()),
        tool_calls: Vec::new(),
    };
    let response_output = sieve_llm::ResponseTurnOutput {
        message: "Handled directly by response writer.".to_string(),
        referenced_ref_ids: BTreeSet::new(),
        summarized_ref_ids: BTreeSet::new(),
    };
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(planner_output)]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalNoToolActionNeeded),
    )]));
    let response: Arc<dyn ResponseModel> =
        Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
    let summary_impl = Arc::new(QueuedSummaryModel::new(vec![Ok(
        "compose should not run".to_string()
    )]));
    let summary: Arc<dyn SummaryModel> = summary_impl.clone();
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
        .run_text_turn("hi")
        .await
        .expect("zero-tool final turn should succeed");

    assert_eq!(
        assistant_messages(&harness.runtime_events()),
        vec!["Handled directly by response writer.".to_string()]
    );
    assert_eq!(
        summary_impl.call_count(),
        0,
        "compose summary model should not run for zero-tool final_no_tool_action_needed turns"
    );
}

#[tokio::test]
async fn e2e_fake_heartbeat_deliver_bypasses_response_and_compose() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some("{\"action\":\"deliver\",\"message\":\"hello\"}".to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalNoToolActionNeeded),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(Vec::new()));
    let summary_impl = Arc::new(QueuedSummaryModel::new(Vec::new()));
    let summary: Arc<dyn SummaryModel> = summary_impl.clone();
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["automation".to_string()],
        E2E_POLICY_BASE,
    );

    let outcome = harness
        .run_prompt_turn(crate::ingress::IngressPrompt {
            source: crate::ingress::PromptSource::Automation,
            session_key: "main".to_string(),
            turn_kind: TurnKind::Heartbeat {
                reason: Some("cron".to_string()),
                queued_event_ids: vec!["evt-1".to_string()],
            },
            text: "queued heartbeat".to_string(),
            modality: InteractionModality::Text,
            media_file_id: None,
        })
        .await
        .expect("heartbeat deliver turn should succeed");

    assert_eq!(outcome.assistant_message, "hello");
    assert!(outcome.assistant_delivered);
    assert!(!outcome.assistant_suppressed_heartbeat_ok);
    assert_eq!(
        assistant_messages(&harness.runtime_events()),
        vec!["hello".to_string()]
    );
    assert_eq!(summary_impl.call_count(), 0);
}

#[tokio::test]
async fn e2e_fake_heartbeat_noop_suppresses_output() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some("{\"action\":\"noop\"}".to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalNoToolActionNeeded),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(Vec::new()));
    let summary_impl = Arc::new(QueuedSummaryModel::new(Vec::new()));
    let summary: Arc<dyn SummaryModel> = summary_impl.clone();
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["automation".to_string()],
        E2E_POLICY_BASE,
    );

    let outcome = harness
        .run_prompt_turn(crate::ingress::IngressPrompt {
            source: crate::ingress::PromptSource::Automation,
            session_key: "main".to_string(),
            turn_kind: TurnKind::Heartbeat {
                reason: Some("cron".to_string()),
                queued_event_ids: vec!["evt-1".to_string()],
            },
            text: "queued heartbeat".to_string(),
            modality: InteractionModality::Text,
            media_file_id: None,
        })
        .await
        .expect("heartbeat noop turn should succeed");

    assert_eq!(outcome.assistant_message, "");
    assert!(!outcome.assistant_delivered);
    assert!(outcome.assistant_suppressed_heartbeat_ok);
    assert!(assistant_messages(&harness.runtime_events()).is_empty());
    assert_eq!(summary_impl.call_count(), 0);
}

#[tokio::test]
async fn e2e_fake_lcm_does_not_auto_inject_trusted_memory_into_planner() {
    let _guard = env_test_lock()
        .lock()
        .expect("lcm recall env test lock poisoned");
    let previous_openai = std::env::var("OPENAI_API_KEY").ok();
    let previous_planner_openai = std::env::var("SIEVE_PLANNER_OPENAI_API_KEY").ok();
    std::env::set_var("OPENAI_API_KEY", "test-openai-key");
    std::env::remove_var("SIEVE_PLANNER_OPENAI_API_KEY");

    let planner: Arc<dyn PlannerModel> = Arc::new(QueuedPlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
    ]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
    ]));
    let response: Arc<dyn ResponseModel> = Arc::new(MemoryRecallResponseModel::new());
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
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

    let mut lcm_config = LcmIntegrationConfig::from_sieve_home(&harness.root);
    lcm_config.enabled = true;
    let lcm = Arc::new(LcmIntegration::new(lcm_config).expect("initialize lcm integration"));
    let harness = harness.with_lcm(Some(lcm));

    harness
        .run_text_turn("Hi I live in Livermore ca")
        .await
        .expect("first memory turn should succeed");
    harness
        .run_text_turn("Where do I live?")
        .await
        .expect("follow-up turn should succeed");

    let assistant = assistant_messages(&harness.runtime_events());
    assert!(
        assistant
            .iter()
            .any(|message| message.contains("I don't know where you live")),
        "without explicit memory tool use, planner should not receive injected trusted memory"
    );

    match previous_openai {
        Some(value) => std::env::set_var("OPENAI_API_KEY", value),
        None => std::env::remove_var("OPENAI_API_KEY"),
    }
    match previous_planner_openai {
        Some(value) => std::env::set_var("SIEVE_PLANNER_OPENAI_API_KEY", value),
        None => std::env::remove_var("SIEVE_PLANNER_OPENAI_API_KEY"),
    }
}

#[tokio::test]
async fn telegram_full_flow_greeting_polls_ingress_and_sends_chat_reply() {
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("chat only".to_string()),
            tool_calls: Vec::new(),
        })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "I'm doing well, thank you!".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
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

    let flow = harness
        .run_telegram_text_turn("Hi how are you?")
        .await
        .expect("telegram full-flow greeting should succeed");

    assert!(
        flow.sent_messages
            .iter()
            .any(|(chat_id, message, _)| *chat_id == 42
                && message.contains("I'm doing well, thank you!")),
        "assistant message should be sent via telegram sendMessage"
    );
    assert!(
        flow.sent_chat_actions
            .iter()
            .any(|(chat_id, action)| *chat_id == 42 && action == "typing"),
        "telegram typing action should be emitted during turn execution"
    );
    assert!(
        !harness
            .runtime_events()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
        "chat-only greeting should not dispatch tools"
    );
}

#[tokio::test]
async fn e2e_fake_natural_language_reminder_can_use_automation_tool() {
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("schedule reminder".to_string()),
            tool_calls: vec![PlannerToolCall {
                tool_name: "automation".to_string(),
                args: BTreeMap::from([
                    ("action".to_string(), serde_json::json!("cron_add")),
                    ("target".to_string(), serde_json::json!("main")),
                    (
                        "schedule".to_string(),
                        serde_json::json!({
                            "kind": "at",
                            "timestamp": "2026-12-01T09:00:00Z"
                        }),
                    ),
                    ("prompt".to_string(), serde_json::json!("say hi")),
                ]),
            }],
        })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "Scheduled it.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["automation".to_string()],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn("Remind me at 2026-12-01T09:00:00Z to say hi")
        .await
        .expect("natural-language reminder turn should succeed");

    assert_eq!(
        assistant_messages(&harness.runtime_events()),
        vec!["Scheduled it.".to_string()]
    );

    let store: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&harness.cfg.automation_store_path)
            .expect("automation store should exist"),
    )
    .expect("parse automation store");
    assert_eq!(store["cron_jobs"]["cron-1"]["prompt"], "say hi");
    assert_eq!(store["cron_jobs"]["cron-1"]["target"], "main");
    assert_eq!(store["cron_jobs"]["cron-1"]["schedule"]["kind"], "at");
    assert_eq!(
        store["cron_jobs"]["cron-1"]["schedule"]["at_ms"],
        serde_json::json!(1_796_115_600_000_u64)
    );
}

#[tokio::test]
async fn e2e_fake_relative_reminder_uses_after_schedule_and_succeeds() {
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some("schedule reminder".to_string()),
            tool_calls: vec![PlannerToolCall {
                tool_name: "automation".to_string(),
                args: BTreeMap::from([
                    ("action".to_string(), serde_json::json!("cron_add")),
                    ("target".to_string(), serde_json::json!("main")),
                    (
                        "schedule".to_string(),
                        serde_json::json!({
                            "kind": "after",
                            "delay": "1m"
                        }),
                    ),
                    ("prompt".to_string(), serde_json::json!("say hi")),
                ]),
            }],
        })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "Scheduled it.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["automation".to_string()],
        E2E_POLICY_BASE,
    );

    let before_ms = crate::logging::now_ms();
    harness
        .run_text_turn("in one minute send me a message saying hi")
        .await
        .expect("relative reminder turn should succeed");
    let after_ms = crate::logging::now_ms();

    assert_eq!(
        assistant_messages(&harness.runtime_events()),
        vec!["Scheduled it.".to_string()]
    );

    let store: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&harness.cfg.automation_store_path)
            .expect("automation store should exist"),
    )
    .expect("parse automation store");
    let at_ms = store["cron_jobs"]["cron-1"]["schedule"]["at_ms"]
        .as_u64()
        .expect("at_ms should be u64");
    assert_eq!(store["cron_jobs"]["cron-1"]["schedule"]["kind"], "at");
    assert!(
        at_ms >= before_ms.saturating_add(60_000),
        "scheduled time should be at least one minute out"
    );
    assert!(
        at_ms <= after_ms.saturating_add(65_000),
        "scheduled time should stay close to one minute out"
    );
}

#[tokio::test]
async fn telegram_full_flow_weather_runs_bash_and_sends_weather_text() {
    let planner: Arc<dyn PlannerModel> = Arc::new(QueuedPlannerModel::new(vec![Ok(
            PlannerTurnOutput {
                thoughts: Some("fetch weather".to_string()),
                tool_calls: vec![PlannerToolCall {
                    tool_name: "bash".to_string(),
                    args: BTreeMap::from([(
                        "cmd".to_string(),
                        serde_json::json!(
                            "echo 'Dublin weather today: 12C and cloudy'; echo 'https://weather.example.test/dublin-today'"
                        ),
                    )]),
                }],
            },
        )]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(FirstStdoutSummaryResponseModel::new());
    let summary: Arc<dyn SummaryModel> = Arc::new(PassThroughSummaryModel);
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

    let flow = harness
        .run_telegram_text_turn("weather in dublin ireland today")
        .await
        .expect("telegram full-flow weather should succeed");

    assert!(
        flow.sent_messages.iter().any(|(_, message, _)| {
            let lower = message.to_ascii_lowercase();
            lower.contains("dublin weather today")
                && lower.contains("12c")
                && message.contains("https://weather.example.test/dublin-today")
        }),
        "assistant telegram reply should include rendered weather result and source URL"
    );
    assert!(
        flow.sent_chat_actions
            .iter()
            .any(|(chat_id, action)| *chat_id == 42 && action == "typing"),
        "telegram typing action should be emitted during weather turn"
    );
    assert!(
        harness
            .runtime_events()
            .iter()
            .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
        "weather request should exercise runtime tool/policy path"
    );
}

#[tokio::test]
async fn e2e_fake_greeting_runs_general_planner_loop_without_tools() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some("friendly response".to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::FinalAnswerReady),
    )]));
    let response_output = sieve_llm::ResponseTurnOutput {
        message: "I'm doing well, thank you!".to_string(),
        referenced_ref_ids: BTreeSet::new(),
        summarized_ref_ids: BTreeSet::new(),
    };
    let response: Arc<dyn ResponseModel> =
        Arc::new(QueuedResponseModel::new(vec![Ok(response_output)]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner: planner.clone(),
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
        .run_text_turn("Hi how are you?")
        .await
        .expect("guided greeting should succeed");

    assert_eq!(
        planner.call_count(),
        1,
        "greeting should still run planner loop once in general mode"
    );
    let events = harness.runtime_events();
    assert_eq!(count_approval_requested(&events), 0);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::PolicyEvaluated(_))),
        "zero-tool greeting should avoid tool policy checks"
    );
}

#[tokio::test]
async fn e2e_fake_guidance_continue_executes_multiple_planner_steps() {
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
        Ok(guidance_output(PlannerGuidanceSignal::ContinueNeedEvidence)),
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
    ]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "Two-step complete.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
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
        .run_text_turn("Gather more context and then answer.")
        .await
        .expect("multi-step guided turn should succeed");

    assert_eq!(
        planner.call_count(),
        2,
        "guidance continue should run step 2"
    );
}

#[tokio::test]
async fn e2e_fake_guidance_continue_stops_after_two_empty_steps() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: Some("step-1".to_string()),
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: Some("step-2".to_string()),
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: Some("step-3".to_string()),
            tool_calls: Vec::new(),
        }),
    ]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
        Ok(guidance_output(PlannerGuidanceSignal::ContinueNeedEvidence)),
        Ok(guidance_output(
            PlannerGuidanceSignal::ContinueFetchAdditionalSource,
        )),
        Ok(guidance_output(
            PlannerGuidanceSignal::ContinueFetchAdditionalSource,
        )),
    ]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "Stopped early.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
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
        .run_text_turn("Keep searching until done.")
        .await
        .expect("empty-step guard turn should succeed");

    assert_eq!(
        planner.call_count(),
        2,
        "two consecutive empty planner steps should stop loop"
    );
}
