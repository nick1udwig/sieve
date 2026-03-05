use super::*;

pub(crate) fn stub_summary() -> CommandSummary {
    CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: "/tmp/test".to_string(),
        }],
        sink_checks: vec![SinkCheck {
            argument_name: "body".to_string(),
            sink: SinkKey("https://example.com/path".to_string()),
            value_refs: vec![ValueRef("v1".to_string())],
        }],
        unsupported_flags: Vec::new(),
    }
}

pub(crate) fn label_with_sinks(integrity: Integrity, sinks: &[&str]) -> ValueLabel {
    let mut provenance = BTreeSet::new();
    provenance.insert(Source::User);
    let allowed_sinks = sinks
        .iter()
        .map(|sink| SinkKey((*sink).to_string()))
        .collect();
    ValueLabel {
        integrity,
        provenance,
        allowed_sinks,
        capacity_type: sieve_types::CapacityType::Enum,
    }
}

pub(crate) fn mk_runtime(
    shell_knowledge: CommandKnowledge,
    segments: Vec<CommandSegment>,
    summary_knowledge: CommandKnowledge,
    policy_kind: PolicyDecisionKind,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<InProcessApprovalBus>,
    Arc<VecEventLog>,
) {
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(StubShell {
            analysis: ShellAnalysis {
                knowledge: shell_knowledge,
                segments,
                unsupported_constructs: Vec::new(),
            },
        }),
        summaries: Arc::new(StubSummaries {
            outcome: SummaryOutcome {
                knowledge: summary_knowledge,
                summary: if summary_knowledge == CommandKnowledge::Known {
                    Some(stub_summary())
                } else {
                    None
                },
                reason: None,
            },
        }),
        policy: Arc::new(StubPolicy {
            decision: PolicyDecision {
                kind: policy_kind,
                reason: "policy verdict".to_string(),
                blocked_rule_id: Some("rule-1".to_string()),
            },
        }),
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
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(DeterministicClock::new(1000)),
    }));
    (runtime, approval_bus, event_log)
}

pub(crate) fn mk_runtime_with_real_summary_and_policy(
    policy_toml: &str,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<InProcessApprovalBus>,
    Arc<VecEventLog>,
) {
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let policy = TomlPolicyEngine::from_toml_str(policy_toml).expect("policy parse");
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
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
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(DeterministicClock::new(1000)),
    }));
    (runtime, approval_bus, event_log)
}

pub(crate) fn mk_runtime_with_capturing_planner(
    planner_output: PlannerTurnOutput,
    shell_knowledge: CommandKnowledge,
    segments: Vec<CommandSegment>,
    summary_knowledge: CommandKnowledge,
    policy_kind: PolicyDecisionKind,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<CapturingPlanner>,
    Arc<InProcessApprovalBus>,
    Arc<VecEventLog>,
) {
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let planner = Arc::new(CapturingPlanner::new(planner_output));
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(StubShell {
            analysis: ShellAnalysis {
                knowledge: shell_knowledge,
                segments,
                unsupported_constructs: Vec::new(),
            },
        }),
        summaries: Arc::new(StubSummaries {
            outcome: SummaryOutcome {
                knowledge: summary_knowledge,
                summary: if summary_knowledge == CommandKnowledge::Known {
                    Some(stub_summary())
                } else {
                    None
                },
                reason: None,
            },
        }),
        policy: Arc::new(StubPolicy {
            decision: PolicyDecision {
                kind: policy_kind,
                reason: "policy verdict".to_string(),
                blocked_rule_id: Some("rule-1".to_string()),
            },
        }),
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
        planner: planner.clone(),
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(DeterministicClock::new(1000)),
    }));
    (runtime, planner, approval_bus, event_log)
}

pub(crate) fn mk_runtime_with_capturing_mainline(
    shell_knowledge: CommandKnowledge,
    segments: Vec<CommandSegment>,
    summary_knowledge: CommandKnowledge,
    policy_kind: PolicyDecisionKind,
    exit_code: Option<i32>,
) -> (
    Arc<RuntimeOrchestrator>,
    Arc<CapturingMainline>,
    Arc<InProcessApprovalBus>,
    Arc<VecEventLog>,
) {
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let event_log = Arc::new(VecEventLog::default());
    let mainline = Arc::new(CapturingMainline::new(exit_code));
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(StubShell {
            analysis: ShellAnalysis {
                knowledge: shell_knowledge,
                segments,
                unsupported_constructs: Vec::new(),
            },
        }),
        summaries: Arc::new(StubSummaries {
            outcome: SummaryOutcome {
                knowledge: summary_knowledge,
                summary: if summary_knowledge == CommandKnowledge::Known {
                    Some(stub_summary())
                } else {
                    None
                },
                reason: None,
            },
        }),
        policy: Arc::new(StubPolicy {
            decision: PolicyDecision {
                kind: policy_kind,
                reason: "policy verdict".to_string(),
                blocked_rule_id: Some("rule-1".to_string()),
            },
        }),
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
        mainline: mainline.clone(),
        planner: Arc::new(StubPlanner {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "gpt-test".to_string(),
                api_base: None,
            },
        }),
        approval_bus: approval_bus.clone(),
        event_log: event_log.clone(),
        clock: Arc::new(DeterministicClock::new(1000)),
    }));
    (runtime, mainline, approval_bus, event_log)
}

pub(crate) async fn wait_for_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
    for _ in 0..20 {
        let published = bus.published_events().expect("published events");
        if let Some(first) = published.first() {
            return first.clone();
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("approval not requested in time");
}

pub(crate) async fn wait_for_approval_count(
    bus: &InProcessApprovalBus,
    count: usize,
) -> Vec<ApprovalRequestedEvent> {
    for _ in 0..20 {
        let published = bus.published_events().expect("published events");
        if published.len() >= count {
            return published;
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("approval count not reached in time");
}
