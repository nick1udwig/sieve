#[tokio::test]
async fn orchestrate_planner_turn_rejects_invalid_tool_args_with_contract_report() {
    let mut args = BTreeMap::new();
    args.insert("cmd".to_string(), json!(""));
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: vec![PlannerToolCall {
            tool_name: "bash".to_string(),
            args,
        }],
    };
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let (runtime, _planner, _approval_bus, _event_log) = mk_runtime_with_capturing_planner(
        planner_output,
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let err = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-1".to_string()),
            cwd: "/tmp".to_string(),
            user_message: "run".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["bash".to_string()],
            current_time_utc: None,
            current_timezone: None,
            allowed_net_connect_scopes: Vec::new(),
            browser_sessions: Vec::new(),
            codex_sessions: Vec::new(),
            previous_events: Vec::new(),
            guidance: None,
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect_err("invalid tool args should fail");

    match err {
        RuntimeError::ToolContract { report } => {
            assert_eq!(report.contract_version, TOOL_CONTRACTS_VERSION);
            assert_eq!(report.errors.len(), 1);
            let validation = &report.errors[0];
            assert_eq!(validation.tool_call_index, 0);
            assert_eq!(validation.tool_name, "bash");
            assert_eq!(validation.argument_path, "/cmd");
        }
        other => panic!("expected tool contract error, got {other:?}"),
    }
}
#[tokio::test]
async fn orchestrate_planner_turn_rejects_disallowed_tool_before_dispatch() {
    let mut args = BTreeMap::new();
    args.insert("cmd".to_string(), json!("echo ok"));
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: vec![PlannerToolCall {
            tool_name: "bash".to_string(),
            args,
        }],
    };
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let (runtime, planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
        planner_output,
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    let err = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-1".to_string()),
            cwd: "/tmp".to_string(),
            user_message: "run echo".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["endorse".to_string()],
            current_time_utc: None,
            current_timezone: None,
            allowed_net_connect_scopes: Vec::new(),
            browser_sessions: Vec::new(),
            codex_sessions: Vec::new(),
            previous_events: Vec::new(),
            guidance: None,
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect_err("disallowed tool should fail");

    match err {
        RuntimeError::DisallowedTool {
            tool_call_index,
            tool_name,
            allowed_tools,
        } => {
            assert_eq!(tool_call_index, 0);
            assert_eq!(tool_name, "bash");
            assert_eq!(allowed_tools, vec!["endorse".to_string()]);
        }
        other => panic!("expected disallowed tool error, got {other:?}"),
    }

    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
    let planner_input = planner.captured_input();
    assert_eq!(planner_input.allowed_tools, vec!["endorse".to_string()]);
}
