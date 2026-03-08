use super::*;

#[tokio::test]
async fn orchestrate_shell_passes_runtime_context_to_policy() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let policy = Arc::new(CapturingPolicy::new(PolicyDecision {
        kind: PolicyDecisionKind::Allow,
        reason: "allow".to_string(),
        blocked_rule_id: None,
    }));

    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(StubShell {
            analysis: ShellAnalysis {
                knowledge: CommandKnowledge::Known,
                segments,
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
        policy: policy.clone(),
        quarantine: Arc::new(StubQuarantine {
            report: QuarantineReport {
                run_id: RunId("run-1".to_string()),
                trace_path: "/tmp/sieve/trace".to_string(),
                stdout_path: None,
                stderr_path: None,
                attempted_capabilities: Vec::new(),
                exit_code: Some(0),
            },
        }),
        mainline: Arc::new(StubMainline),
        planner: Arc::new(StubPlanner {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "gpt-test".to_string(),
                api_base: None,
            },
        }),
        automation: None,
        approval_bus,
        event_log,
        clock: Arc::new(DeterministicClock::new(1000)),
    }));

    runtime
        .upsert_value_label(
            ValueRef("v_control".to_string()),
            label_with_sinks(Integrity::Untrusted, &[]),
        )
        .expect("insert control value label");
    runtime
        .upsert_value_label(
            ValueRef("v_payload".to_string()),
            label_with_sinks(Integrity::Trusted, &["https://example.com/path"]),
        )
        .expect("insert payload value label");

    let mut control_refs = BTreeSet::new();
    control_refs.insert(ValueRef("v_control".to_string()));
    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-1".to_string()),
            cwd: "/tmp".to_string(),
            script: "echo ok".to_string(),
            control_value_refs: control_refs,
            control_endorsed_by: Some(ApprovalRequestId("approval-42".to_string())),
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");
    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-1".to_string()));
            assert_eq!(report.exit_code, Some(0));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }

    let captured = policy.captured_input();
    assert_eq!(
        captured.runtime_context.control.integrity,
        Integrity::Untrusted
    );
    assert_eq!(
        captured.runtime_context.control.endorsed_by,
        Some(ApprovalRequestId("approval-42".to_string()))
    );
    assert!(captured
        .runtime_context
        .control
        .value_refs
        .contains(&ValueRef("v_control".to_string())));
    let sinks = captured
        .runtime_context
        .sink_permissions
        .allowed_sinks_by_value
        .get(&ValueRef("v_payload".to_string()))
        .expect("payload sink permissions");
    assert!(sinks.contains(&SinkKey("https://example.com/path".to_string())));
}

#[tokio::test]
async fn orchestrate_shell_executes_mainline_with_segment_report() {
    let segments = vec![CommandSegment {
        argv: vec!["echo".to_string(), "ok".to_string()],
        operator_before: None,
    }];
    let (runtime, mainline, _approval_bus, _event_log) = mk_runtime_with_capturing_mainline(
        CommandKnowledge::Known,
        segments.clone(),
        CommandKnowledge::Known,
        PolicyDecisionKind::Allow,
        Some(7),
    );

    let disposition = runtime
        .orchestrate_shell(ShellRunRequest {
            run_id: RunId("run-mainline".to_string()),
            cwd: "/tmp".to_string(),
            script: "echo ok".to_string(),
            control_value_refs: BTreeSet::new(),
            control_endorsed_by: None,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        })
        .await
        .expect("runtime ok");

    match disposition {
        RuntimeDisposition::ExecuteMainline(report) => {
            assert_eq!(report.run_id, RunId("run-mainline".to_string()));
            assert_eq!(report.exit_code, Some(7));
        }
        other => panic!("expected mainline execution, got {other:?}"),
    }

    let requests = mainline.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.run_id, RunId("run-mainline".to_string()));
    assert_eq!(request.cwd, "/tmp");
    assert_eq!(request.script, "echo ok");
    assert_eq!(request.command_segments, segments);
}
