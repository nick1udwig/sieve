use sieve_types::{ApprovalRequestedEvent, CodexSessionLifecycleStatus, CodexSessionStatusEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TelegramApprovalAction {
    ApproveOnce,
    ApproveAlways,
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
        "/always" | "/approve_always" | "always" | "approve_always" => {
            TelegramApprovalAction::ApproveAlways
        }
        "/deny" | "deny" => TelegramApprovalAction::Deny,
        _ => return None,
    };

    Some(TelegramApprovalCommand { action, request_id })
}

pub(crate) fn parse_short_action(text: &str) -> Option<TelegramApprovalAction> {
    match text.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" | "👍" => Some(TelegramApprovalAction::ApproveOnce),
        "always" | "a" | "❤️" | "❤" | "♥️" => Some(TelegramApprovalAction::ApproveAlways),
        "no" | "n" | "👎" => Some(TelegramApprovalAction::Deny),
        _ => None,
    }
}

pub(crate) fn parse_reaction_action(emoji: &[String]) -> Option<TelegramApprovalAction> {
    for entry in emoji {
        match entry.as_str() {
            "👍" => return Some(TelegramApprovalAction::ApproveOnce),
            "❤️" | "❤" | "♥️" => return Some(TelegramApprovalAction::ApproveAlways),
            "👎" => return Some(TelegramApprovalAction::Deny),
            _ => {}
        }
    }
    None
}

pub(crate) fn format_approval_requested(event: &ApprovalRequestedEvent) -> String {
    let header = event
        .title
        .as_ref()
        .map(|value| format!("codex agent {value}:\n"))
        .unwrap_or_default();
    let segments = event
        .command_segments
        .iter()
        .map(|segment| segment.argv.join(" "))
        .collect::<Vec<_>>()
        .join(" ; ");
    let preview = event
        .preview
        .as_ref()
        .map(|value| format!("\npreview:\n```diff\n{value}\n```"))
        .unwrap_or_default();
    let decision_help = if event.allow_approve_always {
        "approve once: reply yes/y or react 👍\napprove always: reply always/a or react ❤️\nreject: reply no/n or react 👎"
    } else {
        "approve once: reply yes/y or react 👍\nreject: reply no/n or react 👎"
    };
    if segments.is_empty() {
        format!(
            "{header}approval needed:\n{}\n{preview}\n\n{decision_help}",
            event.reason,
        )
    } else {
        format!(
            "{header}approval needed to run:\n`{}`\n{}\n{preview}\n\n{decision_help}",
            segments, event.reason,
        )
    }
}

pub(crate) fn format_codex_status_card(event: &CodexSessionStatusEvent, now_ms: u64) -> String {
    let status = match event.status {
        CodexSessionLifecycleStatus::Running => "running",
        CodexSessionLifecycleStatus::WaitingApproval => "waiting on approval",
        CodexSessionLifecycleStatus::NeedsFollowup => "needs follow-up",
        CodexSessionLifecycleStatus::Completed => "completed",
        CodexSessionLifecycleStatus::Failed => "failed",
    };
    let elapsed = format_elapsed(now_ms.saturating_sub(event.started_at_ms));
    let cwd = event
        .cwd
        .as_deref()
        .map(|value| format!("repo: `{value}`\n"))
        .unwrap_or_default();
    let summary = event
        .summary
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("\nsummary:\n{value}"))
        .unwrap_or_default();
    format!(
        "codex agent {}:\nstatus: {}\n{}elapsed: {}\nlast step:\n{}{}",
        event.session_name, status, cwd, elapsed, event.last_step, summary
    )
}

fn format_elapsed(elapsed_ms: u64) -> String {
    let total_seconds = elapsed_ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::{
        Action, ApprovalRequestId, ApprovalRequestedEvent, Capability, CodexSessionStatusEvent,
        CommandSegment, Resource, RunId,
    };

    #[test]
    fn parse_command_accepts_bot_mention_suffix() {
        let parsed = parse_command("/approve_once@my_bot approval-1").expect("command parse");
        assert_eq!(parsed.action, TelegramApprovalAction::ApproveOnce);
        assert_eq!(parsed.request_id, "approval-1");
    }

    #[test]
    fn parse_command_supports_approve_always() {
        let parsed = parse_command("/always approval-2").expect("command parse");
        assert_eq!(parsed.action, TelegramApprovalAction::ApproveAlways);
        assert_eq!(parsed.request_id, "approval-2");
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
        assert_eq!(
            parse_short_action("always"),
            Some(TelegramApprovalAction::ApproveAlways)
        );
        assert_eq!(
            parse_short_action("a"),
            Some(TelegramApprovalAction::ApproveAlways)
        );
        assert_eq!(
            parse_short_action("❤️"),
            Some(TelegramApprovalAction::ApproveAlways)
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
            parse_reaction_action(&["❤️".to_string()]),
            Some(TelegramApprovalAction::ApproveAlways)
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
            prompt_kind: sieve_types::ApprovalPromptKind::Command,
            title: None,
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
            preview: None,
            reply_to_session_id: Some("codex-session-1".to_string()),
            allow_approve_always: true,
            created_at_ms: 1,
        });

        assert!(message.contains("approval needed to run:"));
        assert!(message.contains("`rm -rf /tmp/sieve-live-deny-target`"));
        assert!(message.contains("\nrm -rf requires explicit approval"));
        assert!(message.contains("approve once: reply yes/y or react 👍"));
        assert!(message.contains("approve always: reply always/a or react ❤️"));
        assert!(message.contains("reject: reply no/n or react 👎"));
        assert!(!message.contains("request_id:"));
        assert!(!message.contains("blocked_rule_id:"));
    }

    #[test]
    fn format_approval_requested_supports_file_change_preview() {
        let message = format_approval_requested(&ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("approval-2".to_string()),
            run_id: RunId("run-2".to_string()),
            prompt_kind: sieve_types::ApprovalPromptKind::FileChange,
            title: Some("fix-auth-flow".to_string()),
            command_segments: Vec::new(),
            inferred_capabilities: Vec::new(),
            blocked_rule_id: "codex_file_change_approval".to_string(),
            reason: "Codex requested approval to apply file changes".to_string(),
            preview: Some("@@ -1 +1 @@\n-old\n+new".to_string()),
            reply_to_session_id: Some("codex-session-1".to_string()),
            allow_approve_always: true,
            created_at_ms: 2,
        });

        assert!(message.contains("codex agent fix-auth-flow:"));
        assert!(message.contains("approval needed:"));
        assert!(message.contains("```diff"));
        assert!(message.contains("+new"));
    }

    #[test]
    fn format_codex_status_card_renders_running_session() {
        let message = format_codex_status_card(
            &CodexSessionStatusEvent {
                schema_version: 1,
                run_id: RunId("run-1".to_string()),
                session_id: "codex-session-1".to_string(),
                session_name: "you-are-starting".to_string(),
                cwd: Some("/root/git/modex".to_string()),
                status: CodexSessionLifecycleStatus::Running,
                started_at_ms: 1_000,
                updated_at_ms: 61_000,
                last_step: "working in /root/git/modex".to_string(),
                summary: None,
            },
            61_000,
        );

        assert!(message.contains("codex agent you-are-starting:"));
        assert!(message.contains("status: running"));
        assert!(message.contains("repo: `/root/git/modex`"));
        assert!(message.contains("elapsed: 1m 0s"));
        assert!(message.contains("last step:\nworking in /root/git/modex"));
    }
}
