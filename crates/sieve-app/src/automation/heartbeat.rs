use super::{AutomationStore, MAIN_SESSION_KEY};
use chrono::{TimeZone, Utc};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const HEARTBEAT_IDLE_PROMPT: &str = include_str!("../prompts/heartbeat_idle.md");
const HEARTBEAT_EVENTS_PROMPT: &str = include_str!("../prompts/heartbeat_events.md");

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeartbeatPrompt {
    pub(crate) text: String,
    pub(crate) queued_event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub(crate) enum HeartbeatPlannerAction {
    Noop,
    Deliver { message: String },
}

pub(crate) fn parse_heartbeat_planner_action(raw: &str) -> Option<HeartbeatPlannerAction> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(HeartbeatPlannerAction::Noop);
    }
    if trimmed == super::HEARTBEAT_OK_TOKEN {
        return Some(HeartbeatPlannerAction::Noop);
    }
    if let Ok(action) = serde_json::from_str::<HeartbeatPlannerAction>(trimmed) {
        return Some(match action {
            HeartbeatPlannerAction::Deliver { message } if message.trim().is_empty() => {
                HeartbeatPlannerAction::Noop
            }
            other => other,
        });
    }
    Some(HeartbeatPlannerAction::Deliver {
        message: trimmed.to_string(),
    })
}

pub(crate) fn build_heartbeat_prompt(
    store: &AutomationStore,
    now_ms: u64,
    reason: Option<&str>,
    heartbeat_prompt_override: Option<&str>,
    heartbeat_file_path: &Path,
) -> Result<Option<HeartbeatPrompt>, String> {
    let queued_events = store.peek_system_events(MAIN_SESSION_KEY);
    let queued_event_ids = queued_events.iter().map(|event| event.id.clone()).collect();
    let instructions = load_heartbeat_instructions(heartbeat_prompt_override, heartbeat_file_path)?;

    if queued_events.is_empty() && instructions.is_none() {
        return Ok(None);
    }

    let now = Utc
        .timestamp_millis_opt(now_ms as i64)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| now_ms.to_string());

    let prompt = if queued_events.is_empty() {
        HEARTBEAT_IDLE_PROMPT
            .replace("{{NOW}}", &now)
            .replace("{{REASON}}", reason.unwrap_or("interval"))
            .replace(
                "{{INSTRUCTIONS}}",
                instructions.unwrap_or_default().as_str(),
            )
    } else {
        let instructions_block = instructions
            .map(|instructions| format!("\nHeartbeat instructions:\n{instructions}\n"))
            .unwrap_or_default();
        let queued_events_block = queued_events
            .into_iter()
            .map(|event| {
                format!(
                    "- [{}] {}",
                    render_timestamp_ms(event.created_at_ms),
                    event.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        HEARTBEAT_EVENTS_PROMPT
            .replace("{{NOW}}", &now)
            .replace("{{REASON}}", reason.unwrap_or("cron"))
            .replace("{{INSTRUCTIONS_BLOCK}}", &instructions_block)
            .replace("{{QUEUED_EVENTS}}", &queued_events_block)
    };

    Ok(Some(HeartbeatPrompt {
        text: prompt,
        queued_event_ids,
    }))
}

fn load_heartbeat_instructions(
    heartbeat_prompt_override: Option<&str>,
    heartbeat_file_path: &Path,
) -> Result<Option<String>, String> {
    if let Some(override_prompt) = heartbeat_prompt_override {
        let trimmed = override_prompt.trim();
        if !trimmed.is_empty() {
            return Ok(Some(trimmed.to_string()));
        }
    }

    match fs::read_to_string(heartbeat_file_path) {
        Ok(body) => {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "failed reading {}: {err}",
            heartbeat_file_path.display()
        )),
    }
}

fn render_timestamp_ms(timestamp_ms: u64) -> String {
    Utc.timestamp_millis_opt(timestamp_ms as i64)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| timestamp_ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{parse_at_timestamp_ms, AutomationStore, MAIN_SESSION_KEY};
    use std::path::PathBuf;

    fn ts(raw: &str) -> u64 {
        parse_at_timestamp_ms(raw).expect("valid RFC3339")
    }

    #[test]
    fn heartbeat_prompt_uses_file_instructions_when_no_events() {
        let path = PathBuf::from("/tmp/sieve-heartbeat-test-empty");
        let _ = fs::write(&path, "Review status changes");
        let prompt = build_heartbeat_prompt(
            &AutomationStore::default(),
            ts("2026-03-05T10:00:00Z"),
            Some("interval"),
            None,
            &path,
        )
        .expect("prompt")
        .expect("heartbeat prompt");
        assert!(prompt.text.contains("Review status changes"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn heartbeat_prompt_embeds_queued_events() {
        let mut store = AutomationStore::default();
        store.enqueue_system_event(
            MAIN_SESSION_KEY,
            "Reminder: check deploys",
            Some("cron:1"),
            42,
        );
        let prompt = build_heartbeat_prompt(
            &store,
            ts("2026-03-05T10:00:00Z"),
            Some("cron:1"),
            Some("Review the queue."),
            Path::new("/nope"),
        )
        .expect("prompt")
        .expect("heartbeat prompt");
        assert!(prompt.text.contains("Reminder: check deploys"));
        assert!(prompt.text.contains("Review the queue."));
        assert!(prompt
            .text
            .contains("{\"action\":\"deliver\",\"message\":\"...\"}"));
        assert_eq!(prompt.queued_event_ids.len(), 1);
    }

    #[test]
    fn parse_heartbeat_planner_action_handles_json_and_legacy_text() {
        assert_eq!(
            parse_heartbeat_planner_action("{\"action\":\"noop\"}"),
            Some(HeartbeatPlannerAction::Noop)
        );
        assert_eq!(
            parse_heartbeat_planner_action("{\"action\":\"deliver\",\"message\":\"hello\"}"),
            Some(HeartbeatPlannerAction::Deliver {
                message: "hello".to_string(),
            })
        );
        assert_eq!(
            parse_heartbeat_planner_action("hello"),
            Some(HeartbeatPlannerAction::Deliver {
                message: "hello".to_string(),
            })
        );
    }
}
