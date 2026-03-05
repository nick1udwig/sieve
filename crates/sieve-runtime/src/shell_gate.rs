use super::{RuntimeError, RuntimeOrchestrator};
use crate::approval_allowance::{ApprovalAllowanceKey, UnknownOrUncertain};
use crate::MainlineRunReport;
use sieve_command_summaries::SummaryOutcome;
use sieve_types::{
    ApprovalAction, ApprovalRequestId, Capability, CommandKnowledge, CommandSegment,
    CommandSummary, PolicyDecisionKind, PolicyEvaluatedEvent, PrecheckInput,
    QuarantineCompletedEvent, QuarantineReport, QuarantineRunRequest, RunId, RuntimeEvent,
    RuntimePolicyContext, UncertainMode, UnknownMode, ValueRef,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct ShellRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub script: String,
    pub control_value_refs: BTreeSet<ValueRef>,
    pub control_endorsed_by: Option<ApprovalRequestId>,
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDisposition {
    ExecuteMainline(MainlineRunReport),
    ExecuteQuarantine(QuarantineReport),
    Denied { reason: String },
}

impl RuntimeOrchestrator {
    /// Orchestrates shell execution precheck flow.
    /// For composed commands, this executes a single all-or-nothing precheck
    /// and emits one consolidated approval request when needed.
    pub async fn orchestrate_shell(
        &self,
        request: ShellRunRequest,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let runtime_context = self.runtime_policy_context_for_control(
            request.control_value_refs.clone(),
            request.control_endorsed_by.clone(),
        )?;
        let shell = self.shell.analyze_shell_lc_script(&request.script)?;
        let (knowledge, summary) = self.merge_summary(&shell.segments, shell.knowledge);

        if knowledge == CommandKnowledge::Unknown {
            return self
                .handle_unknown_or_uncertain(
                    &request,
                    runtime_context.clone(),
                    shell.segments,
                    UnknownOrUncertain::Unknown,
                )
                .await;
        }

        if knowledge == CommandKnowledge::Uncertain {
            return self
                .handle_unknown_or_uncertain(
                    &request,
                    runtime_context.clone(),
                    shell.segments,
                    UnknownOrUncertain::Uncertain,
                )
                .await;
        }

        let inferred_capabilities = summary
            .as_ref()
            .map(|merged| merged.required_capabilities.clone())
            .unwrap_or_default();
        let precheck = PrecheckInput {
            run_id: request.run_id.clone(),
            cwd: request.cwd.clone(),
            command_segments: shell.segments.clone(),
            knowledge,
            summary,
            runtime_context,
            unknown_mode: request.unknown_mode,
            uncertain_mode: request.uncertain_mode,
        };

        let decision = self.policy.evaluate_precheck(&precheck);
        let policy_event = PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: request.run_id.clone(),
            decision: decision.clone(),
            inferred_capabilities: inferred_capabilities.clone(),
            trace_path: None,
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::PolicyEvaluated(policy_event))
            .await?;

        let command_segments = precheck.command_segments;
        match decision.kind {
            PolicyDecisionKind::Allow => {
                self.execute_mainline(
                    request.run_id.clone(),
                    request.cwd.clone(),
                    request.script.clone(),
                    command_segments,
                )
                .await
            }
            PolicyDecisionKind::Deny => Ok(RuntimeDisposition::Denied {
                reason: decision.reason,
            }),
            PolicyDecisionKind::DenyWithApproval => {
                if decision.blocked_rule_id.as_deref() == Some("missing-capability")
                    && self.capabilities_persistently_allowed(&inferred_capabilities)?
                {
                    return self
                        .execute_mainline(
                            request.run_id,
                            request.cwd,
                            request.script,
                            command_segments,
                        )
                        .await;
                }
                let blocked_rule_id = decision
                    .blocked_rule_id
                    .unwrap_or_else(|| "deny_with_approval".to_string());
                let resolution = self
                    .request_approval(
                        request.run_id.clone(),
                        command_segments.clone(),
                        inferred_capabilities.clone(),
                        blocked_rule_id,
                        decision.reason,
                    )
                    .await?;
                match resolution.action {
                    ApprovalAction::ApproveOnce | ApprovalAction::ApproveAlways => {
                        if resolution.action == ApprovalAction::ApproveAlways {
                            self.remember_persistent_approval_allowances(&inferred_capabilities)?;
                        }
                        self.execute_mainline(
                            request.run_id,
                            request.cwd,
                            request.script,
                            command_segments,
                        )
                        .await
                    }
                    ApprovalAction::Deny => Ok(RuntimeDisposition::Denied {
                        reason: "approval denied".to_string(),
                    }),
                }
            }
        }
    }

    fn merge_summary(
        &self,
        segments: &[CommandSegment],
        shell_knowledge: CommandKnowledge,
    ) -> (CommandKnowledge, Option<CommandSummary>) {
        if shell_knowledge != CommandKnowledge::Known {
            return (shell_knowledge, None);
        }

        let mut merged = CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        };

        for segment in segments {
            let SummaryOutcome {
                knowledge, summary, ..
            } = self.summaries.summarize(&segment.argv);
            match knowledge {
                CommandKnowledge::Known => {
                    let Some(summary) = summary else {
                        return (CommandKnowledge::Unknown, None);
                    };
                    merged
                        .required_capabilities
                        .extend(summary.required_capabilities);
                    merged.sink_checks.extend(summary.sink_checks);
                    merged.unsupported_flags.extend(summary.unsupported_flags);
                }
                CommandKnowledge::Unknown => return (CommandKnowledge::Unknown, None),
                CommandKnowledge::Uncertain => return (CommandKnowledge::Uncertain, None),
            }
        }

        (CommandKnowledge::Known, Some(merged))
    }

    fn remember_persistent_approval_allowances(
        &self,
        capabilities: &[Capability],
    ) -> Result<(), RuntimeError> {
        if capabilities.is_empty() {
            return Ok(());
        }
        let mut allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        for capability in capabilities {
            allowances.insert(ApprovalAllowanceKey::for_capability(capability));
        }
        Ok(())
    }

    fn capabilities_persistently_allowed(
        &self,
        capabilities: &[Capability],
    ) -> Result<bool, RuntimeError> {
        if capabilities.is_empty() {
            return Ok(false);
        }
        let allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        Ok(capabilities
            .iter()
            .map(ApprovalAllowanceKey::for_capability)
            .all(|key| allowances.contains(&key)))
    }

    fn remember_unknown_or_uncertain_allowance(
        &self,
        kind: UnknownOrUncertain,
        command_segments: &[CommandSegment],
    ) -> Result<(), RuntimeError> {
        let mut allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        allowances.insert(ApprovalAllowanceKey::for_unknown_or_uncertain(
            kind,
            command_segments,
        ));
        Ok(())
    }

    fn unknown_or_uncertain_persistently_allowed(
        &self,
        kind: UnknownOrUncertain,
        command_segments: &[CommandSegment],
    ) -> Result<bool, RuntimeError> {
        let allowances = self
            .persistent_approval_allowances
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        Ok(
            allowances.contains(&ApprovalAllowanceKey::for_unknown_or_uncertain(
                kind,
                command_segments,
            )),
        )
    }

    async fn handle_unknown_or_uncertain(
        &self,
        request: &ShellRunRequest,
        runtime_context: RuntimePolicyContext,
        segments: Vec<CommandSegment>,
        kind: UnknownOrUncertain,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let precheck = PrecheckInput {
            run_id: request.run_id.clone(),
            cwd: request.cwd.clone(),
            command_segments: segments.clone(),
            knowledge: kind.to_knowledge(),
            summary: None,
            runtime_context,
            unknown_mode: request.unknown_mode,
            uncertain_mode: request.uncertain_mode,
        };
        let decision = self.policy.evaluate_precheck(&precheck);
        let policy_event = PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: request.run_id.clone(),
            decision: decision.clone(),
            inferred_capabilities: Vec::new(),
            trace_path: None,
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::PolicyEvaluated(policy_event))
            .await?;

        match decision.kind {
            PolicyDecisionKind::Deny => Ok(RuntimeDisposition::Denied {
                reason: decision.reason,
            }),
            PolicyDecisionKind::DenyWithApproval => {
                let blocked_rule_id = kind.to_blocked_rule_id().to_string();
                if self.unknown_or_uncertain_persistently_allowed(kind, &segments)? {
                    let report = self
                        .run_quarantine(request.run_id.clone(), request.cwd.clone(), segments)
                        .await?;
                    return Ok(RuntimeDisposition::ExecuteQuarantine(report));
                }
                let action = self
                    .request_approval(
                        request.run_id.clone(),
                        segments.clone(),
                        Vec::new(),
                        blocked_rule_id,
                        decision.reason,
                    )
                    .await?
                    .action;
                match action {
                    ApprovalAction::ApproveOnce | ApprovalAction::ApproveAlways => {
                        if action == ApprovalAction::ApproveAlways {
                            self.remember_unknown_or_uncertain_allowance(kind, &segments)?;
                        }
                        let report = self
                            .run_quarantine(request.run_id.clone(), request.cwd.clone(), segments)
                            .await?;
                        Ok(RuntimeDisposition::ExecuteQuarantine(report))
                    }
                    ApprovalAction::Deny => Ok(RuntimeDisposition::Denied {
                        reason: "approval denied".to_string(),
                    }),
                }
            }
            PolicyDecisionKind::Allow => {
                let report = self
                    .run_quarantine(request.run_id.clone(), request.cwd.clone(), segments)
                    .await?;
                Ok(RuntimeDisposition::ExecuteQuarantine(report))
            }
        }
    }

    async fn run_quarantine(
        &self,
        run_id: RunId,
        cwd: String,
        command_segments: Vec<CommandSegment>,
    ) -> Result<QuarantineReport, RuntimeError> {
        let report = self
            .quarantine
            .run(QuarantineRunRequest {
                run_id: run_id.clone(),
                cwd,
                command_segments,
            })
            .await?;
        let quarantine_event = QuarantineCompletedEvent {
            schema_version: 1,
            run_id,
            report: report.clone(),
            created_at_ms: self.clock.now_ms(),
        };
        self.append_event(RuntimeEvent::QuarantineCompleted(quarantine_event))
            .await?;
        Ok(report)
    }
}
