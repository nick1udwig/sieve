#[tokio::test]
async fn orchestrate_planner_turn_dispatches_automation_tool() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("schedule reminder".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "automation".to_string(),
            args: BTreeMap::from([
                ("action".to_string(), json!("cron_add")),
                ("target".to_string(), json!("main")),
                (
                    "schedule".to_string(),
                    json!({
                        "kind": "at",
                        "timestamp": "2026-12-01T09:00:00Z"
                    }),
                ),
                ("prompt".to_string(), json!("say hi")),
            ]),
        }],
    };
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let automation = Arc::new(CapturingAutomation::new("Scheduled cron-1."));
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(StubShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Known,
                segments: Vec::new(),
                unsupported_constructs: Vec::new(),
            },
        }),
        summaries: Arc::new(StubSummaries {
            outcome: SummaryOutcome {
                knowledge: CommandKnowledge::Known,
                summary: Some(stub_summary()),
                reason: None,
            },
        }),
        policy: Arc::new(StubPolicy {
            decision: PolicyDecision {
                kind: PolicyDecisionKind::Allow,
                reason: "allow".to_string(),
                blocked_rule_id: None,
            },
        }),
        quarantine: Arc::new(StubQuarantine {
            report: QuarantineReport {
                run_id: RunId("run-automation".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner: planner.clone(),
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
        automation: Some(automation.clone()),
        codex: None,
    }));

    let output = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-automation".to_string()),
            cwd: "/tmp".to_string(),
            user_message: "remind me at 2026-12-01T09:00:00Z to say hi".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["automation".to_string()],
            available_commands: Vec::new(),
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
        .expect("runtime planner turn");

    assert_eq!(output.thoughts, Some("schedule reminder".to_string()));
    assert_eq!(
        automation.requests(),
        vec![AutomationRequest {
            action: AutomationAction::CronAdd,
            target: Some(AutomationTarget::Main),
            schedule: Some(AutomationSchedule::At {
                timestamp: "2026-12-01T09:00:00Z".to_string(),
            }),
            prompt: Some("say hi".to_string()),
            job_id: None,
        }]
    );
    assert_eq!(output.tool_results.len(), 1);
    match &output.tool_results[0] {
        PlannerToolResult::Automation {
            request,
            message,
            effect,
            failure_reason,
        } => {
            assert_eq!(
                request,
                &AutomationRequest {
                    action: AutomationAction::CronAdd,
                    target: Some(AutomationTarget::Main),
                    schedule: Some(AutomationSchedule::At {
                        timestamp: "2026-12-01T09:00:00Z".to_string(),
                    }),
                    prompt: Some("say hi".to_string()),
                    job_id: None,
                }
            );
            assert_eq!(message.as_deref(), Some("Scheduled cron-1."));
            assert_eq!(effect, &None);
            assert_eq!(failure_reason, &None);
        }
        other => panic!("expected automation result, got {other:?}"),
    }
    let planner_input = planner.captured_input();
    assert_eq!(planner_input.allowed_tools, vec!["automation".to_string()]);
}
#[tokio::test]
async fn orchestrate_planner_turn_keeps_automation_argument_failures_recoverable() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("schedule reminder".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "automation".to_string(),
            args: BTreeMap::from([
                ("action".to_string(), json!("cron_add")),
                ("target".to_string(), json!("main")),
                (
                    "schedule".to_string(),
                    json!({
                        "kind": "at",
                        "timestamp": "in 1 minute"
                    }),
                ),
                ("prompt".to_string(), json!("say hi")),
            ]),
        }],
    };
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let automation = Arc::new(FailingAutomation::new(
        "timestamp must be RFC3339 or unix-ms",
    ));
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(StubShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Known,
                segments: Vec::new(),
                unsupported_constructs: Vec::new(),
            },
        }),
        summaries: Arc::new(StubSummaries {
            outcome: SummaryOutcome {
                knowledge: CommandKnowledge::Known,
                summary: Some(stub_summary()),
                reason: None,
            },
        }),
        policy: Arc::new(StubPolicy {
            decision: PolicyDecision {
                kind: PolicyDecisionKind::Allow,
                reason: "allow".to_string(),
                blocked_rule_id: None,
            },
        }),
        quarantine: Arc::new(StubQuarantine {
            report: QuarantineReport {
                run_id: RunId("run-automation-failure".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner: planner.clone(),
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
        automation: Some(automation.clone()),
        codex: None,
    }));

    let output = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-automation-failure".to_string()),
            cwd: "/tmp".to_string(),
            user_message: "remind me in one minute to say hi".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["automation".to_string()],
            available_commands: Vec::new(),
            current_time_utc: Some("2026-03-08T06:30:00Z".to_string()),
            current_timezone: Some("UTC".to_string()),
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
        .expect("runtime planner turn");

    assert_eq!(
        automation.requests(),
        vec![AutomationRequest {
            action: AutomationAction::CronAdd,
            target: Some(AutomationTarget::Main),
            schedule: Some(AutomationSchedule::At {
                timestamp: "in 1 minute".to_string(),
            }),
            prompt: Some("say hi".to_string()),
            job_id: None,
        }]
    );
    assert_eq!(output.tool_results.len(), 1);
    match &output.tool_results[0] {
        PlannerToolResult::Automation {
            request,
            message,
            effect,
            failure_reason,
        } => {
            assert_eq!(
                request,
                &AutomationRequest {
                    action: AutomationAction::CronAdd,
                    target: Some(AutomationTarget::Main),
                    schedule: Some(AutomationSchedule::At {
                        timestamp: "in 1 minute".to_string(),
                    }),
                    prompt: Some("say hi".to_string()),
                    job_id: None,
                }
            );
            assert_eq!(message, &None);
            assert_eq!(effect, &None);
            assert_eq!(
                failure_reason.as_deref(),
                Some("timestamp must be RFC3339 or unix-ms")
            );
        }
        other => panic!("expected automation result, got {other:?}"),
    }

    let planner_input = planner.captured_input();
    assert_eq!(
        planner_input.current_time_utc.as_deref(),
        Some("2026-03-08T06:30:00Z")
    );
    assert_eq!(planner_input.current_timezone.as_deref(), Some("UTC"));
}
