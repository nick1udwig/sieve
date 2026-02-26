use sieve_types::{
    ApprovalRequestedEvent, PolicyDecisionKind, PolicyEvaluatedEvent, QuarantineCompletedEvent,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TelegramApprovalAction {
    ApproveOnce,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TelegramApprovalCommand {
    pub(crate) action: TelegramApprovalAction,
    pub(crate) request_id: String,
}

pub(crate) fn parse_command(text: &str) -> Option<TelegramApprovalCommand> {
    let mut parts = text.split_whitespace();
    let command = parts.next()?;
    let request_id = parts.next()?.to_string();
    if parts.next().is_some() {
        return None;
    }

    let action = match command {
        "/approve" | "/approve_once" | "approve" | "approve_once" => {
            TelegramApprovalAction::ApproveOnce
        }
        "/deny" | "deny" => TelegramApprovalAction::Deny,
        _ => return None,
    };

    Some(TelegramApprovalCommand { action, request_id })
}

pub(crate) fn format_approval_requested(event: &ApprovalRequestedEvent) -> String {
    let segments = event
        .command_segments
        .iter()
        .map(|segment| segment.argv.join(" "))
        .collect::<Vec<_>>()
        .join(" ; ");

    let capabilities = if event.inferred_capabilities.is_empty() {
        "none".to_string()
    } else {
        event
            .inferred_capabilities
            .iter()
            .map(|cap| format!("{:?}.{:?} {}", cap.resource, cap.action, cap.scope))
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "approval requested\nrequest_id: {}\nrun_id: {}\nargv: {}\ncapabilities: {}\nblocked_rule_id: {}\nreason: {}",
        event.request_id.0,
        event.run_id.0,
        segments,
        capabilities,
        event.blocked_rule_id,
        event.reason
    )
}

pub(crate) fn format_policy_evaluated(event: &PolicyEvaluatedEvent) -> String {
    let decision = match event.decision.kind {
        PolicyDecisionKind::Allow => "allow",
        PolicyDecisionKind::DenyWithApproval => "deny_with_approval",
        PolicyDecisionKind::Deny => "deny",
    };

    format!(
        "policy evaluated\nrun_id: {}\ndecision: {}\nreason: {}",
        event.run_id.0, decision, event.decision.reason
    )
}

pub(crate) fn format_quarantine_completed(event: &QuarantineCompletedEvent) -> String {
    format!(
        "quarantine completed\nrun_id: {}\ntrace_path: {}\nexit_code: {:?}",
        event.run_id.0, event.report.trace_path, event.report.exit_code
    )
}
