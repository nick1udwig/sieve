use super::*;

#[tokio::test]
async fn orchestrate_planner_turn_executes_bash_through_policy_and_approval() {
    let mut args = BTreeMap::new();
    args.insert("cmd".to_string(), json!("rm -rf tmp"));
    let planner_output = PlannerTurnOutput {
        thoughts: Some("run approved command".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "bash".to_string(),
            args,
        }],
    };
    let segments = vec![CommandSegment {
        argv: vec!["rm".to_string(), "-rf".to_string(), "tmp".to_string()],
        operator_before: None,
    }];
    let (runtime, planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
        planner_output,
        CommandKnowledge::Known,
        segments,
        CommandKnowledge::Known,
        PolicyDecisionKind::DenyWithApproval,
    );

    let previous_events = vec![RuntimeEvent::ApprovalResolved(ApprovalResolvedEvent {
        schema_version: 1,
        request_id: ApprovalRequestId("approval-prev".to_string()),
        run_id: RunId("run-prev".to_string()),
        action: ApprovalAction::Deny,
        created_at_ms: 900,
    })];

    let runtime_task = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: RunId("run-1".to_string()),
                    cwd: "/tmp".to_string(),
                    user_message: "delete tmp".to_string(),
                    allowed_tools: vec!["bash".to_string()],
                    allowed_net_connect_scopes: Vec::new(),
                    previous_events,
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
    assert_eq!(requested.blocked_rule_id, "rule-1");
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

    assert_eq!(output.thoughts, Some("run approved command".to_string()));
    assert_eq!(output.tool_results.len(), 1);
    match &output.tool_results[0] {
        PlannerToolResult::Bash {
            command,
            disposition,
        } => {
            assert_eq!(command, "rm -rf tmp");
            match disposition {
                RuntimeDisposition::ExecuteMainline(report) => {
                    assert_eq!(report.run_id, RunId("run-1".to_string()));
                    assert_eq!(report.exit_code, Some(0));
                }
                other => panic!("expected mainline execution, got {other:?}"),
            }
        }
        other => panic!("expected bash result, got {other:?}"),
    }

    let planner_input = planner.captured_input();
    assert_eq!(planner_input.run_id, RunId("run-1".to_string()));
    assert_eq!(planner_input.user_message, "delete tmp");
    assert_eq!(planner_input.allowed_tools, vec!["bash".to_string()]);
    assert_eq!(planner_input.previous_events.len(), 1);
}

#[tokio::test]
async fn approve_always_whitelists_missing_capability_by_net_origin() {
    let (runtime, approval_bus, _event_log) = mk_runtime_with_real_summary_and_policy(
        r#"
[options]
violation_mode = "ask"
require_trusted_control_for_mutating = false
trusted_control = true
"#,
    );

    let first_run = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .orchestrate_shell(ShellRunRequest {
                    run_id: RunId("run-allow-always-1".to_string()),
                    cwd: "/tmp".to_string(),
                    script: "curl https://example.com/one".to_string(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: UnknownMode::Deny,
                    uncertain_mode: UncertainMode::Deny,
                })
                .await
        })
    };
    let first_requested = wait_for_approval_count(&approval_bus, 1).await[0].clone();
    approval_bus
        .resolve(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: first_requested.request_id.clone(),
            run_id: first_requested.run_id.clone(),
            action: ApprovalAction::ApproveAlways,
            created_at_ms: 1001,
        })
        .expect("resolve first approval as always");
    let first_disposition = first_run
        .await
        .expect("first run task join")
        .expect("first run");
    assert!(matches!(
        first_disposition,
        RuntimeDisposition::ExecuteMainline(_)
    ));

    let second_disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-allow-always-2".to_string()),
            cwd: "/tmp".to_string(),
            script: "curl https://example.com/two".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("second run");
    assert!(matches!(
        second_disposition,
        RuntimeDisposition::ExecuteMainline(_)
    ));
    assert_eq!(
        approval_bus
            .published_events()
            .expect("published approvals")
            .len(),
        1
    );
}

#[test]
fn restore_persistent_approval_allowances_normalizes_net_connect_scope_to_origin() {
    let (runtime, _approval_bus, _event_log) = mk_runtime(
        CommandKnowledge::Known,
        vec![CommandSegment {
            argv: vec!["echo".to_string(), "ok".to_string()],
            operator_before: None,
        }],
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
    );

    runtime
        .restore_persistent_approval_allowances(&[Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://example.com/path?x=1".to_string(),
        }])
        .expect("restore allowances");

    let allowances = runtime
        .persistent_approval_allowances()
        .expect("snapshot allowances");
    assert_eq!(allowances.len(), 1);
    assert_eq!(
        allowances[0],
        Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://example.com".to_string(),
        }
    );
}

#[tokio::test]
async fn orchestrate_planner_turn_runs_unknown_bash_in_quarantine_when_accepted() {
    let mut args = BTreeMap::new();
    args.insert("cmd".to_string(), json!("custom-cmd --flag"));
    let planner_output = PlannerTurnOutput {
        thoughts: None,
        tool_calls: vec![PlannerToolCall {
            tool_name: "bash".to_string(),
            args,
        }],
    };
    let segments = vec![CommandSegment {
        argv: vec!["custom-cmd".to_string(), "--flag".to_string()],
        operator_before: None,
    }];
    let (runtime, _planner, approval_bus, _event_log) = mk_runtime_with_capturing_planner(
        planner_output,
        CommandKnowledge::Unknown,
        segments,
        CommandKnowledge::Unknown,
        PolicyDecisionKind::Allow,
    );

    let output = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-1".to_string()),
            cwd: "/tmp".to_string(),
            user_message: "run custom command".to_string(),
            allowed_tools: vec!["bash".to_string()],
            allowed_net_connect_scopes: Vec::new(),
            previous_events: Vec::new(),
            guidance: None,
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Accept,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime planner turn");

    assert_eq!(output.tool_results.len(), 1);
    match &output.tool_results[0] {
        PlannerToolResult::Bash { disposition, .. } => {
            assert!(matches!(
                disposition,
                RuntimeDisposition::ExecuteQuarantine(_)
            ));
        }
        other => panic!("expected bash result, got {other:?}"),
    }
    assert!(approval_bus
        .published_events()
        .expect("published events")
        .is_empty());
}

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
            allowed_tools: vec!["bash".to_string()],
            allowed_net_connect_scopes: Vec::new(),
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
            allowed_tools: vec!["endorse".to_string()],
            allowed_net_connect_scopes: Vec::new(),
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
                    allowed_tools: vec!["endorse".to_string()],
                    allowed_net_connect_scopes: Vec::new(),
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
