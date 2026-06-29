#[tokio::test]
async fn orchestrate_planner_turn_executes_endorse_with_approval() {
    let mut args = BTreeMap::new();
    args.insert("value_ref".to_string(), json!("v_control"));
    args.insert("target_integrity".to_string(), json!("trusted"));
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: vec![PlannerToolCall {
            tool_name: "endorse".to_string(),
            args,
        }],
    };
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let (runtime, _planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
        planner_output,
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );
    runtime
        .upsert_value_label(
            ValueRef("v_control".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("seed value state");

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: RunId("run-1".to_string()),
                    cwd: "/tmp".to_string(),
                    user_message: "endorse control".to_string(),
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
        })
    };

    let requested = wait_for_approval(&approval_bus).await;
    assert_eq!(requested.command_segments[0].argv[0], "endorse");
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: requested.request_id.clone(),
            run_id: requested.run_id.clone(),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1001,
        })
        .expect("resolve approval");

    let output = runtime_task
        .await
        .expect("task join")
        .expect("runtime planner turn");
    assert_eq!(output.tool_results.len(), 1);
    match &output.tool_results[0] {
        PlannerToolResult::Endorse {
            request,
            transition: Some(transition),
        } => {
            assert_eq!(request.value_ref, ValueRef("v_control".to_string()));
            assert_eq!(transition.to_integrity, Integrity::Trusted);
            assert_eq!(transition.approved_by, Some(requested.request_id));
        }
        other => panic!("expected endorse transition, got {other:?}"),
    }

    let label = runtime
        .value_label(&ValueRef("v_control".to_string()))
        .expect("read value label")
        .expect("value label present");
    assert_eq!(label.integrity, Integrity::Trusted);
}
