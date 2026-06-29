use crate::logging::{now_ms, ConversationLogRecord, ConversationRole, FanoutRuntimeEventLog};
use sieve_runtime::{EventLogError, RuntimeEventLog};
use sieve_types::{AssistantMessageEvent, RunId, RuntimeEvent};

pub(in crate::turn) async fn emit_assistant_error_message(
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    error_message: String,
) -> Result<(), EventLogError> {
    event_log
        .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: run_id.clone(),
            message: error_message.clone(),
            reply_to_session_id: None,
            created_at_ms: now_ms(),
        }))
        .await?;
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            error_message,
            now_ms(),
        ))
        .await
}
