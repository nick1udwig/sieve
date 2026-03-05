use super::{ApprovalResolution, RuntimeError, RuntimeOrchestrator};
use sieve_types::{
    ApprovalAction, CommandKnowledge, CommandSegment, CommandSummary, DeclassifyRequest,
    DeclassifyStateTransition, EndorseRequest, EndorseStateTransition, PolicyDecision,
    PolicyDecisionKind, PrecheckInput, RunId, UncertainMode, UnknownMode,
};
use std::collections::BTreeSet;

impl RuntimeOrchestrator {
    /// Starts approval lifecycle for an explicit `endorse` tool request.
    /// Returns one-shot approval action; no persistent allowlist state.
    pub async fn request_endorse_approval(
        &self,
        run_id: RunId,
        request: EndorseRequest,
    ) -> Result<ApprovalAction, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "endorse".to_string(),
                request.value_ref.0,
                format!("{:?}", request.target_integrity).to_lowercase(),
            ],
            operator_before: None,
        };
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "endorse_requires_approval",
                "endorse requires approval",
            )
            .await?
        else {
            return Ok(ApprovalAction::Deny);
        };

        Ok(resolution.action)
    }

    /// Runs `endorse` approval and applies state transition when approved once.
    pub async fn endorse_value_once(
        &self,
        run_id: RunId,
        request: EndorseRequest,
    ) -> Result<Option<EndorseStateTransition>, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "endorse".to_string(),
                request.value_ref.0.clone(),
                format!("{:?}", request.target_integrity).to_lowercase(),
            ],
            operator_before: None,
        };
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "endorse_requires_approval",
                "endorse requires approval",
            )
            .await?
        else {
            return Ok(None);
        };

        if resolution.action == ApprovalAction::Deny {
            return Ok(None);
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
        Ok(Some(transition))
    }

    /// Starts approval lifecycle for an explicit `declassify` tool request.
    /// Returns one-shot approval action; no persistent allowlist state.
    pub async fn request_declassify_approval(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
    ) -> Result<ApprovalAction, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "declassify".to_string(),
                request.value_ref.0,
                request.sink.0,
            ],
            operator_before: None,
        };
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "declassify_requires_approval",
                "declassify requires approval",
            )
            .await?
        else {
            return Ok(ApprovalAction::Deny);
        };

        Ok(resolution.action)
    }

    /// Runs `declassify` approval and applies state transition when approved once.
    pub async fn declassify_value_once(
        &self,
        run_id: RunId,
        request: DeclassifyRequest,
    ) -> Result<Option<DeclassifyStateTransition>, RuntimeError> {
        let segment = CommandSegment {
            argv: vec![
                "declassify".to_string(),
                request.value_ref.0.clone(),
                request.sink.0.clone(),
            ],
            operator_before: None,
        };
        let Some(resolution) = self
            .approve_explicit_tool_call(
                run_id,
                segment,
                "declassify_requires_approval",
                "declassify requires approval",
            )
            .await?
        else {
            return Ok(None);
        };

        if resolution.action == ApprovalAction::Deny {
            return Ok(None);
        }

        let mut state = self
            .value_state
            .lock()
            .map_err(|_| crate::ValueStateError::LockPoisoned)?;
        let transition = state.apply_declassify_transition(
            request.value_ref,
            request.sink,
            Some(resolution.request_id),
        )?;
        Ok(Some(transition))
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
        fallback_blocked_rule_id: &str,
        fallback_reason: &str,
    ) -> Result<Option<ApprovalResolution>, RuntimeError> {
        let decision = self.evaluate_explicit_tool_policy(&run_id, &segment)?;
        match decision.kind {
            PolicyDecisionKind::Deny => Ok(None),
            PolicyDecisionKind::Allow => {
                let resolution = self
                    .approve_tool_call(run_id, segment, fallback_blocked_rule_id, fallback_reason)
                    .await?;
                Ok(Some(resolution))
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
                Ok(Some(resolution))
            }
        }
    }

    fn evaluate_explicit_tool_policy(
        &self,
        run_id: &RunId,
        segment: &CommandSegment,
    ) -> Result<PolicyDecision, RuntimeError> {
        let runtime_context = self.runtime_policy_context_for_control(BTreeSet::new(), None)?;
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
