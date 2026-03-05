use super::*;
use async_trait::async_trait;

pub(crate) struct StubShell {
    pub(crate) analysis: ShellAnalysis,
}

impl ShellAnalyzer for StubShell {
    fn analyze_shell_lc_script(&self, _script: &str) -> Result<ShellAnalysis, ShellAnalysisError> {
        Ok(self.analysis.clone())
    }
}

pub(crate) struct StubSummaries {
    pub(crate) outcome: SummaryOutcome,
}

impl CommandSummarizer for StubSummaries {
    fn summarize(&self, _argv: &[String]) -> SummaryOutcome {
        self.outcome.clone()
    }
}

pub(crate) struct StubPolicy {
    pub(crate) decision: PolicyDecision,
}

impl PolicyEngine for StubPolicy {
    fn evaluate_precheck(&self, input: &PrecheckInput) -> PolicyDecision {
        match input.knowledge {
            CommandKnowledge::Unknown => match input.unknown_mode {
                UnknownMode::Deny => PolicyDecision {
                    kind: PolicyDecisionKind::Deny,
                    reason: "unknown command denied by mode".to_string(),
                    blocked_rule_id: Some("unknown-mode".to_string()),
                },
                UnknownMode::Ask => PolicyDecision {
                    kind: PolicyDecisionKind::DenyWithApproval,
                    reason: "unknown command requires approval".to_string(),
                    blocked_rule_id: Some("unknown-mode".to_string()),
                },
                UnknownMode::Accept => PolicyDecision {
                    kind: PolicyDecisionKind::Allow,
                    reason: "unknown command accepted by mode".to_string(),
                    blocked_rule_id: None,
                },
            },
            CommandKnowledge::Uncertain => match input.uncertain_mode {
                UncertainMode::Deny => PolicyDecision {
                    kind: PolicyDecisionKind::Deny,
                    reason: "uncertain command denied by mode".to_string(),
                    blocked_rule_id: Some("uncertain-mode".to_string()),
                },
                UncertainMode::Ask => PolicyDecision {
                    kind: PolicyDecisionKind::DenyWithApproval,
                    reason: "uncertain command requires approval".to_string(),
                    blocked_rule_id: Some("uncertain-mode".to_string()),
                },
                UncertainMode::Accept => PolicyDecision {
                    kind: PolicyDecisionKind::Allow,
                    reason: "uncertain command accepted by mode".to_string(),
                    blocked_rule_id: None,
                },
            },
            CommandKnowledge::Known => self.decision.clone(),
        }
    }
}

pub(crate) struct CapturingPolicy {
    decision: PolicyDecision,
    last_input: StdMutex<Option<PrecheckInput>>,
}

impl CapturingPolicy {
    pub(crate) fn new(decision: PolicyDecision) -> Self {
        Self {
            decision,
            last_input: StdMutex::new(None),
        }
    }

    pub(crate) fn captured_input(&self) -> PrecheckInput {
        self.last_input
            .lock()
            .expect("policy lock")
            .clone()
            .expect("captured precheck input")
    }
}

impl PolicyEngine for CapturingPolicy {
    fn evaluate_precheck(&self, input: &PrecheckInput) -> PolicyDecision {
        *self.last_input.lock().expect("policy lock") = Some(input.clone());
        self.decision.clone()
    }
}

pub(crate) struct StubQuarantine {
    pub(crate) report: QuarantineReport,
}

#[async_trait]
impl QuarantineRunner for StubQuarantine {
    async fn run(
        &self,
        _request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError> {
        Ok(self.report.clone())
    }
}

pub(crate) struct StubMainline;

#[async_trait]
impl MainlineRunner for StubMainline {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: Some(0),
            artifacts: Vec::new(),
        })
    }
}

pub(crate) struct CapturingMainline {
    exit_code: Option<i32>,
    requests: StdMutex<Vec<MainlineRunRequest>>,
}

impl CapturingMainline {
    pub(crate) fn new(exit_code: Option<i32>) -> Self {
        Self {
            exit_code,
            requests: StdMutex::new(Vec::new()),
        }
    }

    pub(crate) fn requests(&self) -> Vec<MainlineRunRequest> {
        self.requests.lock().expect("mainline lock").clone()
    }
}

#[async_trait]
impl MainlineRunner for CapturingMainline {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        self.requests
            .lock()
            .map_err(|_| MainlineRunError::Exec("mainline lock poisoned".to_string()))?
            .push(request.clone());
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: self.exit_code,
            artifacts: Vec::new(),
        })
    }
}

pub(crate) struct StubPlanner {
    pub(crate) config: LlmModelConfig,
}

#[async_trait]
impl PlannerModel for StubPlanner {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, _input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        })
    }
}

pub(crate) struct CapturingPlanner {
    config: LlmModelConfig,
    output: PlannerTurnOutput,
    last_input: StdMutex<Option<PlannerTurnInput>>,
}

impl CapturingPlanner {
    pub(crate) fn new(output: PlannerTurnOutput) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "gpt-test".to_string(),
                api_base: None,
            },
            output,
            last_input: StdMutex::new(None),
        }
    }

    pub(crate) fn captured_input(&self) -> PlannerTurnInput {
        self.last_input
            .lock()
            .expect("planner lock")
            .clone()
            .expect("captured planner input")
    }
}

#[async_trait]
impl PlannerModel for CapturingPlanner {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        *self.last_input.lock().expect("planner lock") = Some(input);
        Ok(self.output.clone())
    }
}

pub(crate) struct DeterministicClock {
    now: AtomicU64,
}

impl DeterministicClock {
    pub(crate) fn new(start: u64) -> Self {
        Self {
            now: AtomicU64::new(start),
        }
    }
}

impl Clock for DeterministicClock {
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Default)]
pub(crate) struct VecEventLog {
    events: StdMutex<Vec<RuntimeEvent>>,
}

#[async_trait]
impl RuntimeEventLog for VecEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        self.events
            .lock()
            .map_err(|_| EventLogError::Append("test lock poisoned".to_string()))?
            .push(event);
        Ok(())
    }
}

impl VecEventLog {
    pub(crate) fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.events.lock().expect("event lock").clone()
    }
}
