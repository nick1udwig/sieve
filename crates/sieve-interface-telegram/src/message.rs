use sieve_types::ApprovalRequestedEvent;

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

pub(crate) fn parse_short_action(text: &str) -> Option<TelegramApprovalAction> {
    match text.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" => Some(TelegramApprovalAction::ApproveOnce),
        "no" | "n" => Some(TelegramApprovalAction::Deny),
        _ => None,
    }
}

pub(crate) fn parse_reaction_action(emoji: &[String]) -> Option<TelegramApprovalAction> {
    for entry in emoji {
        match entry.as_str() {
            "👍" => return Some(TelegramApprovalAction::ApproveOnce),
            "👎" => return Some(TelegramApprovalAction::Deny),
            _ => {}
        }
    }
    None
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
        "approval needed\nrequest_id: {}\nrun_id: {}\ncommand: {}\ncapabilities: {}\nblocked_rule_id: {}\nreason: {}\n\napprove: reply yes/y or react 👍\nreject: reply no/n or react 👎\nalt: /approve_once {} or /deny {}",
        event.request_id.0,
        event.run_id.0,
        segments,
        capabilities,
        event.blocked_rule_id,
        event.reason,
        event.request_id.0,
        event.request_id.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_short_action_supports_yes_and_no() {
        assert_eq!(
            parse_short_action("yes"),
            Some(TelegramApprovalAction::ApproveOnce)
        );
        assert_eq!(
            parse_short_action("Y"),
            Some(TelegramApprovalAction::ApproveOnce)
        );
        assert_eq!(parse_short_action("n"), Some(TelegramApprovalAction::Deny));
        assert_eq!(parse_short_action("maybe"), None);
    }

    #[test]
    fn parse_reaction_action_supports_thumb_reactions() {
        assert_eq!(
            parse_reaction_action(&["👍".to_string()]),
            Some(TelegramApprovalAction::ApproveOnce)
        );
        assert_eq!(
            parse_reaction_action(&["👎".to_string()]),
            Some(TelegramApprovalAction::Deny)
        );
        assert_eq!(
            parse_reaction_action(&["✨".to_string(), "👍".to_string()]),
            Some(TelegramApprovalAction::ApproveOnce)
        );
        assert_eq!(parse_reaction_action(&["✨".to_string()]), None);
    }
}
