use super::orchestrator::ApprovalResolution;
use super::{RuntimeError, RuntimeOrchestrator};
use sieve_types::{
    ApprovalAction, ApprovalRequestId, CapacityType, CommandKnowledge, CommandSegment,
    CommandSummary, DeclassifyRequest, DeclassifyStateTransition, EndorseRequest,
    EndorseStateTransition, Integrity, PolicyDecision, PolicyDecisionKind, PrecheckInput, RunId,
    UncertainMode, UnknownMode, ValueRef,
};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy)]
enum ExplicitToolKind {
    Endorse { target_integrity: Integrity },
    Declassify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplicitToolApplyOutcome<T> {
    pub transition: Option<T>,
    pub failure_reason: Option<String>,
}

enum ExplicitToolGateResolution {
    PolicyDenied { reason: String },
    ApprovalResolved(ApprovalResolution),
}

impl RuntimeOrchestrator {
    /// Starts approval lifecycle for an explicit `endorse` tool request.
    /// Returns one-shot approval action; no persistent allowlist state.
    pub async fn request_endorse_approval(
        &self,
        run_id: RunId,
        request: EndorseRequest,
    ) -> Result<ApprovalAction, RuntimeError> {
        self.request_endorse_approval_with_context(run_id, request, BTreeSet::new(), None)
            .await
    }

    pub async fn request_endorse_approval_with_context(
        &self,
        run_id: RunId,
        request: EndorseRequest,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<ApprovalAction, RuntimeError> {
        let value_ref = request.value_ref.clone();
        let segment = CommandSegment {
            argv: vec![
                "endorse".to_string(),
                value_ref.0.clone(),
                format!("{:?}", request.target_integrity).to_lowercase(),
            ],
            operator_before: None,
        };
        let resolution = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                value_ref,
                ExplicitToolKind::Endorse {
                    target_integrity: request.target_integrity,
                },
                control_value_refs,
                control_endorsed_by,
                "endorse_requires_approval",
                "endorse requires approval",
            )
            .await?;
        match resolution {
            ExplicitToolGateResolution::PolicyDenied { .. } => Ok(ApprovalAction::Deny),
            ExplicitToolGateResolution::ApprovalResolved(resolution) => Ok(resolution.action),
        }
    }

    /// Runs `endorse` approval and applies state transition when approved once.
    pub async fn endorse_value_once(
        &self,
        run_id: RunId,
        request: EndorseRequest,
    ) -> Result<Option<EndorseStateTransition>, RuntimeError> {
        self.endorse_value_once_with_context(run_id, request, BTreeSet::new(), None)
            .await
    }

    pub async fn endorse_value_once_with_context(
        &self,
        run_id: RunId,
        request: EndorseRequest,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<Option<EndorseStateTransition>, RuntimeError> {
        Ok(self
            .endorse_value_once_outcome_with_context(
                run_id,
                request,
                control_value_refs,
                control_endorsed_by,
            )
            .await?
            .transition)
    }

    pub(crate) async fn endorse_value_once_outcome_with_context(
        &self,
        run_id: RunId,
        request: EndorseRequest,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<ExplicitToolApplyOutcome<EndorseStateTransition>, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "endorse".to_string(),
                request.value_ref.0.clone(),
                format!("{:?}", request.target_integrity).to_lowercase(),
            ],
            operator_before: None,
        };
        let resolution = match self
            .approve_explicit_tool_call(
                run_id,
                segment,
                request.value_ref.clone(),
                ExplicitToolKind::Endorse {
                    target_integrity: request.target_integrity,
                },
                control_value_refs,
                control_endorsed_by,
                "endorse_requires_approval",
                "endorse requires approval",
            )
            .await
        {
            Ok(ExplicitToolGateResolution::PolicyDenied { reason }) => {
                return Ok(ExplicitToolApplyOutcome {
                    transition: None,
                    failure_reason: Some(reason),
                });
            }
            Ok(ExplicitToolGateResolution::ApprovalResolved(resolution)) => resolution,
            Err(RuntimeError::ValueState(crate::ValueStateError::UnknownValueRef(value_ref))) => {
                return Ok(ExplicitToolApplyOutcome {
                    transition: None,
                    failure_reason: Some(format!("unknown value ref: {value_ref}")),
                });
            }
            Err(err) => return Err(err),
        };

        if resolution.action == ApprovalAction::Deny {
            return Ok(ExplicitToolApplyOutcome {
                transition: None,
                failure_reason: Some("approval denied".to_string()),
            });
        }

        let mut state = self
            .value_state
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        let transition = state.apply_endorse_transition(
            request.value_ref,
            request.target_integrity,
            Some(resolution.request_id),
        )?;
        Ok(ExplicitToolApplyOutcome {
            transition: Some(transition),
            failure_reason: None,
        })
    }

    /// Starts approval lifecycle for an explicit `declassify` tool request.
    /// Returns one-shot approval action; no persistent allowlist state.
    pub async fn request_declassify_approval(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
    ) -> Result<ApprovalAction, RuntimeError> {
        self.request_declassify_approval_with_context(run_id, request, BTreeSet::new(), None)
            .await
    }

    pub async fn request_declassify_approval_with_context(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<ApprovalAction, RuntimeError> {
        let value_ref = request.value_ref.clone();
        let segment = CommandSegment {
            argv: vec![
                "declassify".to_string(),
                value_ref.0.clone(),
                request.sink.0.clone(),
                format!("{:?}", request.channel).to_lowercase(),
            ],
            operator_before: None,
        };
        let resolution = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                value_ref,
                ExplicitToolKind::Declassify,
                control_value_refs,
                control_endorsed_by,
                "declassify_requires_approval",
                "declassify requires approval",
            )
            .await?;
        match resolution {
            ExplicitToolGateResolution::PolicyDenied { .. } => Ok(ApprovalAction::Deny),
            ExplicitToolGateResolution::ApprovalResolved(resolution) => Ok(resolution.action),
        }
    }

    /// Runs `declassify` approval and applies state transition when approved once.
    pub async fn declassify_value_once(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
    ) -> Result<Option<DeclassifyStateTransition>, RuntimeError> {
        self.declassify_value_once_with_context(run_id, request, BTreeSet::new(), None)
            .await
    }

    pub async fn declassify_value_once_with_context(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<Option<DeclassifyStateTransition>, RuntimeError> {
        Ok(self
            .declassify_value_once_outcome_with_context(
                run_id,
                request,
                control_value_refs,
                control_endorsed_by,
            )
            .await?
            .transition)
    }

    pub(crate) async fn declassify_value_once_outcome_with_context(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<ExplicitToolApplyOutcome<DeclassifyStateTransition>, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "declassify".to_string(),
                request.value_ref.0.clone(),
                request.sink.0.clone(),
                format!("{:?}", request.channel).to_lowercase(),
            ],
            operator_before: None,
        };
        let resolution = match self
            .approve_explicit_tool_call(
                run_id,
                segment,
                request.value_ref.clone(),
                ExplicitToolKind::Declassify,
                control_value_refs,
                control_endorsed_by,
                "declassify_requires_approval",
                "declassify requires approval",
            )
            .await
        {
            Ok(ExplicitToolGateResolution::PolicyDenied { reason }) => {
                return Ok(ExplicitToolApplyOutcome {
                    transition: None,
                    failure_reason: Some(reason),
                });
            }
            Ok(ExplicitToolGateResolution::ApprovalResolved(resolution)) => resolution,
            Err(RuntimeError::ValueState(crate::ValueStateError::UnknownValueRef(value_ref))) => {
                return Ok(ExplicitToolApplyOutcome {
                    transition: None,
                    failure_reason: Some(format!("unknown value ref: {value_ref}")),
                });
            }
            Err(err) => return Err(err),
        };

        if resolution.action == ApprovalAction::Deny {
            return Ok(ExplicitToolApplyOutcome {
                transition: None,
                failure_reason: Some("approval denied".to_string()),
            });
        }

        let mut state = self
            .value_state
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        let transition = state.apply_declassify_transition(
            request.value_ref,
            request.sink,
            request.channel,
            Some(resolution.request_id),
        )?;
        Ok(ExplicitToolApplyOutcome {
            transition: Some(transition),
            failure_reason: None,
        })
    }

    async fn approve_tool_call(
        &self,
        run_id: RunId,
        segment: CommandSegment,
        blocked_rule_id: &str,
        reason: &str,
    ) -> Result<ApprovalResolution, RuntimeError> {
        self.request_approval(
            run_id,
            vec![segment],
            Vec::new(),
            blocked_rule_id.to_string(),
            reason.to_string(),
        )
        .await
    }

    async fn approve_explicit_tool_call(
        &self,
        run_id: RunId,
        segment: CommandSegment,
        value_ref: ValueRef,
        tool_kind: ExplicitToolKind,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
        fallback_blocked_rule_id: &str,
        fallback_reason: &str,
    ) -> Result<ExplicitToolGateResolution, RuntimeError> {
        let decision = self.evaluate_explicit_tool_policy(
            &run_id,
            &segment,
            &value_ref,
            tool_kind,
            control_value_refs,
            control_endorsed_by,
        )?;
        match decision.kind {
            PolicyDecisionKind::Deny => Ok(ExplicitToolGateResolution::PolicyDenied {
                reason: decision.reason,
            }),
            PolicyDecisionKind::Allow => {
                let resolution = self
                    .approve_tool_call(run_id, segment, fallback_blocked_rule_id, fallback_reason)
                    .await?;
                Ok(ExplicitToolGateResolution::ApprovalResolved(resolution))
            }
            PolicyDecisionKind::DenyWithApproval => {
                let blocked_rule_id = decision
                    .blocked_rule_id
                    .unwrap_or_else(|| fallback_blocked_rule_id.to_string());
                let resolution = self
                    .request_approval(
                        run_id,
                        vec![segment],
                        Vec::new(),
                        blocked_rule_id,
                        decision.reason,
                    )
                    .await?;
                Ok(ExplicitToolGateResolution::ApprovalResolved(resolution))
            }
        }
    }

    fn evaluate_explicit_tool_policy(
        &self,
        run_id: &RunId,
        segment: &CommandSegment,
        value_ref: &ValueRef,
        tool_kind: ExplicitToolKind,
        control_value_refs: BTreeSet<ValueRef>,
        control_endorsed_by: Option<ApprovalRequestId>,
    ) -> Result<PolicyDecision, RuntimeError> {
        let label = self
            .value_label(value_ref)?
            .ok_or_else(|| crate::ValueStateError::UnknownValueRef(value_ref.0.clone()))?;
        if let Some(decision) = explicit_tool_capacity_decision(tool_kind, label.capacity_type) {
            return Ok(decision);
        }
        let runtime_context =
            self.runtime_policy_context_for_control(control_value_refs, control_endorsed_by)?;
        if runtime_context.control.integrity == Integrity::Untrusted {
            return Ok(PolicyDecision {
                kind: PolicyDecisionKind::DenyWithApproval,
                reason: "untrusted control context for explicit tool action".to_string(),
                blocked_rule_id: Some("integrity-untrusted-control".to_string()),
            });
        }
        let input = PrecheckInput {
            run_id: run_id.clone(),
            cwd: ".".to_string(),
            command_segments: vec![segment.clone()],
            knowledge: CommandKnowledge::Known,
            summary: Some(CommandSummary {
                required_capabilities: Vec::new(),
                sink_checks: Vec::new(),
                unsupported_flags: Vec::new(),
            }),
            runtime_context,
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        };
        Ok(self.policy.evaluate_precheck(&input))
    }
}

fn explicit_tool_capacity_decision(
    tool_kind: ExplicitToolKind,
    capacity_type: CapacityType,
) -> Option<PolicyDecision> {
    if capacity_type != CapacityType::TrustedString {
        return None;
    }

    match tool_kind {
        ExplicitToolKind::Endorse {
            target_integrity: Integrity::Trusted,
        } => Some(PolicyDecision {
            kind: PolicyDecisionKind::Deny,
            reason: "trusted_string values require typed extraction before endorse".to_string(),
            blocked_rule_id: Some("capacity-trusted-string-endorse".to_string()),
        }),
        ExplicitToolKind::Declassify => Some(PolicyDecision {
            kind: PolicyDecisionKind::Deny,
            reason: "trusted_string values require typed extraction before declassify".to_string(),
            blocked_rule_id: Some("capacity-trusted-string-declassify".to_string()),
        }),
        ExplicitToolKind::Endorse { .. } => None,
    }
}
