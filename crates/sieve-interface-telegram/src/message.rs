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
    let command_raw = parts.next()?;
    let command = command_raw
        .split_once('@')
        .map(|(base, _)| base)
        .unwrap_or(command_raw);
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
        "yes" | "y" | "👍" => Some(TelegramApprovalAction::ApproveOnce),
        "no" | "n" | "👎" => Some(TelegramApprovalAction::Deny),
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

    format!(
        "approval needed to run:\n`{}`\nbecause {}\n\napprove: reply yes/y or react 👍\nreject: reply no/n or react 👎",
        segments,
        event.reason,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::{
        Action, ApprovalRequestId, ApprovalRequestedEvent, Capability, CommandSegment, Resource,
        RunId,
    };

    #[test]
    fn parse_command_accepts_bot_mention_suffix() {
        let parsed = parse_command("/approve_once@my_bot approval-1").expect("command parse");
        assert_eq!(parsed.action, TelegramApprovalAction::ApproveOnce);
        assert_eq!(parsed.request_id, "approval-1");
    }

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
        assert_eq!(
            parse_short_action("👍"),
            Some(TelegramApprovalAction::ApproveOnce)
        );
        assert_eq!(parse_short_action("n"), Some(TelegramApprovalAction::Deny));
        assert_eq!(parse_short_action("👎"), Some(TelegramApprovalAction::Deny));
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

    #[test]
    fn format_approval_requested_uses_minimal_human_copy() {
        let message = format_approval_requested(&ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-1".to_string()),
            run_id: RunId("run-1".to_string()),
            command_segments: vec![CommandSegment {
                argv: vec![
                    "rm".to_string(),
                    "-rf".to_string(),
                    "/tmp/sieve-live-deny-target".to_string(),
                ],
                operator_before: None,
            }],
            inferred_capabilities: vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/sieve-live-deny-target".to_string(),
            }],
            blocked_rule_id: "deny-rm-rf".to_string(),
            reason: "rm -rf requires explicit approval".to_string(),
            created_at_ms: 1,
        });

        assert!(message.contains("approval needed to run:"));
        assert!(message.contains("`rm -rf /tmp/sieve-live-deny-target`"));
        assert!(message.contains("because rm -rf requires explicit approval"));
        assert!(message.contains("approve: reply yes/y or react 👍"));
        assert!(message.contains("reject: reply no/n or react 👎"));
        assert!(!message.contains("request_id:"));
        assert!(!message.contains("blocked_rule_id:"));
    }
}
