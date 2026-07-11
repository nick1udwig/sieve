#[tokio::test]
async fn orchestrate_planner_turn_dispatches_codex_exec_tool() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("run git status in codex sandbox".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "codex_exec".to_string(),
            args: BTreeMap::from([
                ("command".to_string(), json!(["git", "status"])),
                ("sandbox".to_string(), json!("read_only")),
                ("cwd".to_string(), json!("/tmp/repo")),
                ("timeout_ms".to_string(), json!(5000)),
            ]),
        }],
    };
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let codex = Arc::new(CapturingCodex::new(
        Ok(CodexExecToolResult {
            result: CodexExecResult {
                exit_code: 0,
                stdout: "On branch main".to_string(),
                stderr: String::new(),
            },
        }),
        Err("unused".to_string()),
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
                run_id: RunId("run-codex-exec".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner: planner.clone(),
        automation: None,
        codex: Some(codex.clone()),
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
    }));

    let output = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-codex-exec".to_string()),
            cwd: "/tmp/repo".to_string(),
            user_message: "check git status".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["codex_exec".to_string()],
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

    assert_eq!(
        output.thoughts,
        Some("run git status in codex sandbox".to_string())
    );
    assert_eq!(
        codex.exec_requests(),
        vec![CodexExecRequest {
            command: vec!["git".to_string(), "status".to_string()],
            sandbox: CodexSandboxMode::ReadOnly,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: Vec::new(),
            timeout_ms: Some(5000),
        }]
    );
    match &output.tool_results[0] {
        PlannerToolResult::CodexExec {
            request,
            result,
            failure_reason,
        } => {
            assert_eq!(
                request.command,
                vec!["git".to_string(), "status".to_string()]
            );
            assert_eq!(
                result,
                &Some(CodexExecResult {
                    exit_code: 0,
                    stdout: "On branch main".to_string(),
                    stderr: String::new(),
                })
            );
            assert!(failure_reason.is_none());
        }
        other => panic!("expected codex exec result, got {other:?}"),
    }
}
#[tokio::test]
async fn orchestrate_planner_turn_dispatches_codex_session_tool() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("resume codex session".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "codex_session".to_string(),
            args: BTreeMap::from([
                ("session_id".to_string(), json!("fix-auth-flow")),
                (
                    "instruction".to_string(),
                    json!("continue from the last completed phase"),
                ),
                ("sandbox".to_string(), json!("workspace_write")),
                ("cwd".to_string(), json!("/tmp/repo")),
            ]),
        }],
    };
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let codex = Arc::new(CapturingCodex::new(
        Err("unused".to_string()),
        Ok(CodexSessionToolResult {
            result: CodexTurnResult {
                session_id: Some("fix-auth-flow".to_string()),
                session_name: "fix-auth-flow".to_string(),
                status: CodexTurnStatus::Completed,
                summary: "updated the auth flow and tests".to_string(),
                user_visible: Some("Implemented the auth flow fix.".to_string()),
                turn_id: Some("turn_123".to_string()),
                thread_id: Some("thr_123".to_string()),
            },
        }),
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
                run_id: RunId("run-codex".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner: planner.clone(),
        automation: None,
        codex: Some(codex.clone()),
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
    }));

    let output = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-codex".to_string()),
            cwd: "/tmp/repo".to_string(),
            user_message: "continue the codex implementation".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["codex_session".to_string()],
            available_commands: Vec::new(),
            current_time_utc: None,
            current_timezone: None,
            allowed_net_connect_scopes: Vec::new(),
            browser_sessions: Vec::new(),
            codex_sessions: vec![PlannerCodexSession {
                session_id: "fix-auth-flow".to_string(),
                session_name: "fix-auth-flow".to_string(),
                cwd: "/tmp/repo".to_string(),
                sandbox: CodexSandboxMode::WorkspaceWrite,
                updated_at_utc: "2026-03-09T12:00:00Z".to_string(),
                status: "completed".to_string(),
                task_summary: "fix auth flow".to_string(),
                last_result_summary: Some("updated parser".to_string()),
            }],
            previous_events: Vec::new(),
            guidance: None,
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime planner turn");

    assert_eq!(codex.session_requests().len(), 1);
    assert_eq!(
        codex.session_requests()[0],
        CodexSessionRequest {
            session_id: Some("fix-auth-flow".to_string()),
            instruction: "continue from the last completed phase".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: Vec::new(),
            local_images: Vec::new(),
        }
    );
    match &output.tool_results[0] {
        PlannerToolResult::CodexSession {
            request,
            result,
            failure_reason,
        } => {
            assert_eq!(request.session_id.as_deref(), Some("fix-auth-flow"));
            assert_eq!(
                result.as_ref().map(|value| value.summary.as_str()),
                Some("updated the auth flow and tests")
            );
            assert_eq!(failure_reason, &None);
        }
        other => panic!("expected codex session result, got {other:?}"),
    }
}
#[tokio::test]
async fn orchestrate_planner_turn_defaults_codex_session_cwd_to_runtime_cwd() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("resume codex session".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "codex_session".to_string(),
            args: BTreeMap::from([
                ("session_id".to_string(), json!("fix-auth-flow")),
                (
                    "instruction".to_string(),
                    json!("continue from the last completed phase"),
                ),
                ("sandbox".to_string(), json!("workspace_write")),
            ]),
        }],
    };
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let codex = Arc::new(CapturingCodex::new(
        Err("unused".to_string()),
        Ok(CodexSessionToolResult {
            result: CodexTurnResult {
                session_id: Some("fix-auth-flow".to_string()),
                session_name: "fix-auth-flow".to_string(),
                status: CodexTurnStatus::Completed,
                summary: "updated the auth flow and tests".to_string(),
                user_visible: Some("Implemented the auth flow fix.".to_string()),
                turn_id: Some("turn_123".to_string()),
                thread_id: Some("thr_123".to_string()),
            },
        }),
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
                run_id: RunId("run-codex".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner,
        automation: None,
        codex: Some(codex.clone()),
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
    }));

    runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-codex".to_string()),
            cwd: "/tmp/repo".to_string(),
            user_message: "continue the codex session".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["codex_session".to_string()],
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
            unknown_mode: UnknownMode::Ask,
            uncertain_mode: UncertainMode::Ask,
        })
        .await
        .expect("runtime planner turn");

    assert_eq!(
        codex.session_requests(),
        vec![CodexSessionRequest {
            session_id: Some("fix-auth-flow".to_string()),
            instruction: "continue from the last completed phase".to_string(),
            sandbox: CodexSandboxMode::WorkspaceWrite,
            cwd: Some("/tmp/repo".to_string()),
            writable_roots: Vec::new(),
            local_images: Vec::new(),
        }]
    );
}
#[tokio::test]
async fn orchestrate_planner_turn_keeps_codex_session_failures_recoverable() {
    let planner_output = PlannerTurnOutput {
        thoughts: Some("start codex session".to_string()),
        tool_calls: vec![PlannerToolCall {
            tool_name: "codex_session".to_string(),
            args: BTreeMap::from([
                ("instruction".to_string(), json!("start the project")),
                ("sandbox".to_string(), json!("workspace_write")),
                ("cwd".to_string(), json!("~/git/modex")),
            ]),
        }],
    };
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let codex = Arc::new(CapturingCodex::new(
        Err("unused".to_string()),
        Err("thread/name/set timed out after 100ms".to_string()),
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
                run_id: RunId("run-codex-fail".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner: planner.clone(),
        automation: None,
        codex: Some(codex.clone()),
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
    }));

    let output = runtime
        .orchestrate_planner_turn(PlannerRunRequest {
            run_id: RunId("run-codex-fail".to_string()),
            cwd: "/tmp/repo".to_string(),
            user_message: "start the codex project".to_string(),
            conversation: Vec::new(),
            allowed_tools: vec!["codex_session".to_string()],
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

    match &output.tool_results[0] {
        PlannerToolResult::CodexSession {
            request,
            result,
            failure_reason,
        } => {
            assert_eq!(request.cwd.as_deref(), Some("~/git/modex"));
            assert!(result.is_none());
            assert_eq!(
                failure_reason.as_deref(),
                Some("thread/name/set timed out after 100ms")
            );
        }
        other => panic!("expected codex session failure result, got {other:?}"),
    }
}
