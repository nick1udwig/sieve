use crate::text_match::{contains_any_word, contains_phrase_words, contains_token_sequence};
use sieve_runtime::{PlannerRunResult, PlannerToolResult};
use sieve_types::PlannerCodexSession;

pub(super) fn looks_like_codex_status_query(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_phrase_words(&lower, &["how", "is"])
        || contains_phrase_words(&lower, &["how", "s"])
        || contains_any_word(
            &lower,
            &[
                "status",
                "doing",
                "done",
                "ongoing",
                "progress",
                "finished",
                "complete",
                "completed",
            ],
        )
}

pub(super) fn find_referenced_codex_session<'a>(
    message: &str,
    sessions: &'a [PlannerCodexSession],
) -> Option<&'a PlannerCodexSession> {
    let lower = message.to_ascii_lowercase();
    sessions
        .iter()
        .filter_map(|session| {
            let mut score = 0usize;
            if contains_token_sequence(&lower, &session.session_name) {
                score = score.max(100);
            }
            if let Some(name) = session
                .cwd
                .rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
            {
                if contains_token_sequence(&lower, name) {
                    score = score.max(80);
                }
            }
            if contains_token_sequence(&lower, &session.task_summary) {
                score = score.max(60);
            }
            (score > 0).then_some((score, session))
        })
        .max_by_key(|(score, session)| (*score, session.updated_at_utc.as_str()))
        .map(|(_, session)| session)
}

pub(super) fn format_codex_session_status_reply(session: &PlannerCodexSession) -> String {
    match session.status.as_str() {
        "completed" => format!(
            "It’s done, not ongoing. The saved session `{}` is marked `completed`, last updated at `{}`, and its summary says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("the work completed")
        ),
        "failed" => format!(
            "It failed. The saved session `{}` is marked `failed`, last updated at `{}`, and the latest summary says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("the last Codex turn failed")
        ),
        "needs_followup" => format!(
            "It is not done yet. The saved session `{}` needs follow-up, last updated at `{}`, and the latest summary says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("more work remains")
        ),
        "waiting_approval" => format!(
            "It’s waiting on approval right now. The saved session `{}` last updated at `{}`, and the latest status says {}.",
            session.session_name,
            session.updated_at_utc,
            session
                .last_result_summary
                .as_deref()
                .unwrap_or("Codex requested approval before it can continue")
        ),
        _ => format!(
            "It’s still running. The saved session `{}` is marked `{}`, last updated at `{}`, and the task summary is {}.",
            session.session_name,
            session.status,
            session.updated_at_utc,
            session.task_summary
        ),
    }
}

pub(super) fn reply_to_codex_session_id(
    trusted_user_message: &str,
    aggregated_result: &PlannerRunResult,
    codex_sessions: &[PlannerCodexSession],
) -> Option<String> {
    aggregated_result
        .tool_results
        .iter()
        .rev()
        .find_map(|tool_result| match tool_result {
            PlannerToolResult::CodexSession {
                result: Some(result),
                ..
            } => result.session_id.clone(),
            _ => None,
        })
        .or_else(|| {
            find_referenced_codex_session(trusted_user_message, codex_sessions)
                .map(|session| session.session_id.clone())
        })
}
