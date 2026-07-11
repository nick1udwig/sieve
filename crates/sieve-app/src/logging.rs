mod event_hub;
mod projection;

use async_trait::async_trait;
pub(crate) use event_hub::RuntimeEventHub;
use projection::InMemoryEventProjection;
#[cfg(test)]
pub(crate) use projection::{
    MAX_RETAINED_CONVERSATION_ENTRIES, MAX_RETAINED_RUNTIME_EVENTS, MAX_RETAINED_TURN_CONTEXTS,
};
use serde::Serialize;
use sieve_runtime::{EventLogError, JsonlRuntimeEventLog, RuntimeEventLog};
use sieve_types::{
    ApprovalRequestedEvent, ApprovalResolvedEvent, AssistantMessageEvent, CodexSessionStatusEvent,
    PolicyEvaluatedEvent, QuarantineCompletedEvent, RunId, RuntimeEvent,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConversationRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ConversationLogRecord {
    event: &'static str,
    schema_version: u16,
    run_id: RunId,
    role: ConversationRole,
    message: String,
    created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConversationHistoryEntry {
    pub(crate) run_id: RunId,
    pub(crate) session_key: String,
    pub(crate) role: ConversationRole,
    pub(crate) message: String,
    pub(crate) created_at_ms: u64,
}

impl ConversationLogRecord {
    pub(crate) fn new(
        run_id: RunId,
        role: ConversationRole,
        message: String,
        created_at_ms: u64,
    ) -> Self {
        Self {
            event: "conversation",
            schema_version: 1,
            run_id,
            role,
            message,
            created_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TelegramLoopEvent {
    Runtime(Box<RuntimeEvent>),
    TypingStart { run_id: String },
    TypingStop { run_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReservedTurn {
    pub(crate) run_id: RunId,
    pub(crate) turn_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnLogContext {
    turn_seq: u64,
    source: String,
    session_key: String,
    turn_kind: String,
}

#[derive(Serialize)]
struct TurnScopedRecord<'a, T: Serialize> {
    schema_version: u16,
    event: &'a str,
    component: &'a str,
    level: &'a str,
    created_at_ms: u64,
    session_id: &'a str,
    turn_id: &'a str,
    payload: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "logical_session_key"
    )]
    session_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_kind: Option<String>,
}

#[derive(Serialize)]
struct ConversationPayload<'a> {
    role: ConversationRole,
    message: &'a str,
}

#[derive(Serialize)]
struct ApprovalRequestedPayload<'a> {
    request_id: &'a str,
    command_segments: &'a [sieve_types::CommandSegment],
    inferred_capabilities: &'a [sieve_types::Capability],
    blocked_rule_id: &'a str,
    reason: &'a str,
    reply_to_session_id: &'a Option<String>,
}

#[derive(Serialize)]
struct ApprovalResolvedPayload<'a> {
    request_id: &'a str,
    action: &'a sieve_types::ApprovalAction,
}

#[derive(Serialize)]
struct PolicyEvaluatedPayload<'a> {
    decision: &'a sieve_types::PolicyDecision,
    inferred_capabilities: &'a [sieve_types::Capability],
    trace_path: &'a Option<String>,
}

#[derive(Serialize)]
struct QuarantineCompletedPayload<'a> {
    report: &'a sieve_types::QuarantineReport,
}

#[derive(Serialize)]
struct CodexSessionStatusPayload<'a> {
    session_id: &'a str,
    session_name: &'a str,
    cwd: &'a Option<String>,
    status: &'a sieve_types::CodexSessionLifecycleStatus,
    started_at_ms: u64,
    updated_at_ms: u64,
    last_step: &'a str,
    summary: &'a Option<String>,
}

#[derive(Serialize)]
struct AssistantMessagePayload<'a> {
    message: &'a str,
    reply_to_session_id: &'a Option<String>,
}

pub(crate) struct FanoutRuntimeEventLog {
    jsonl: JsonlRuntimeEventLog,
    session_id: String,
    next_turn_seq: AtomicU64,
    projection: InMemoryEventProjection,
    event_hub: RuntimeEventHub,
}

impl FanoutRuntimeEventLog {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        event_hub: impl Into<RuntimeEventHub>,
    ) -> Result<Self, EventLogError> {
        Self::with_session_id(path, event_hub, fresh_session_id())
    }

    pub(crate) fn with_session_id(
        path: impl Into<PathBuf>,
        event_hub: impl Into<RuntimeEventHub>,
        session_id: String,
    ) -> Result<Self, EventLogError> {
        Ok(Self {
            jsonl: JsonlRuntimeEventLog::new(path.into())?,
            session_id,
            next_turn_seq: AtomicU64::new(1),
            projection: InMemoryEventProjection::new(),
            event_hub: event_hub.into(),
        })
    }

    #[cfg(test)]
    pub(crate) fn reserve_turn(&self, source: &str) -> ReservedTurn {
        self.reserve_turn_with_metadata(source, "main", "user")
    }

    pub(crate) fn reserve_turn_with_metadata(
        &self,
        source: &str,
        session_key: &str,
        turn_kind: &str,
    ) -> ReservedTurn {
        let turn_seq = self.next_turn_seq.fetch_add(1, Ordering::Relaxed);
        let run_id = RunId(format!("{}-t{}", self.session_id, turn_seq));
        self.projection.insert_turn_context(
            run_id.clone(),
            TurnLogContext {
                turn_seq,
                source: source.to_string(),
                session_key: session_key.to_string(),
                turn_kind: turn_kind.to_string(),
            },
        );
        ReservedTurn { run_id, turn_seq }
    }

    pub(crate) fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.projection.runtime_snapshot()
    }

    pub(crate) fn snapshot_conversation(&self, session_key: &str) -> Vec<ConversationHistoryEntry> {
        self.projection.conversation_snapshot(session_key)
    }

    pub(crate) async fn append_conversation(
        &self,
        record: ConversationLogRecord,
    ) -> Result<(), EventLogError> {
        let value = self.turn_scoped_record(
            "conversation",
            "conversation",
            "info",
            &record.run_id,
            record.created_at_ms,
            &ConversationPayload {
                role: record.role,
                message: &record.message,
            },
        )?;
        self.jsonl.append_json_value(&value).await?;
        let session_key = self
            .projection
            .turn_context(&record.run_id)?
            .as_ref()
            .map(|context| context.session_key.clone())
            .unwrap_or_else(|| "main".to_string());
        self.projection
            .push_conversation_entry(ConversationHistoryEntry {
                run_id: record.run_id,
                session_key,
                role: record.role,
                message: record.message,
                created_at_ms: record.created_at_ms,
            })?;
        Ok(())
    }

    pub(crate) async fn append_app_event<T: Serialize>(
        &self,
        component: &str,
        event: &str,
        level: &str,
        run_id: &RunId,
        created_at_ms: u64,
        payload: T,
    ) -> Result<(), EventLogError> {
        let value =
            self.turn_scoped_record(event, component, level, run_id, created_at_ms, &payload)?;
        self.jsonl.append_json_value(&value).await
    }

    fn turn_scoped_record<T: Serialize>(
        &self,
        event: &str,
        component: &str,
        level: &str,
        run_id: &RunId,
        created_at_ms: u64,
        payload: &T,
    ) -> Result<serde_json::Value, EventLogError> {
        let context = self.projection.turn_context(run_id)?;
        serde_json::to_value(TurnScopedRecord {
            schema_version: 2,
            event,
            component,
            level,
            created_at_ms,
            session_id: &self.session_id,
            turn_id: &run_id.0,
            payload,
            turn_seq: context.as_ref().map(|value| value.turn_seq),
            source: context.as_ref().map(|value| value.source.clone()),
            session_key: context.as_ref().map(|value| value.session_key.clone()),
            turn_kind: context.as_ref().map(|value| value.turn_kind.clone()),
        })
        .map_err(|err| EventLogError::Append(format!("failed to encode event record: {err}")))
    }

    fn runtime_event_record(
        &self,
        event: &RuntimeEvent,
    ) -> Result<serde_json::Value, EventLogError> {
        match event {
            RuntimeEvent::ApprovalRequested(ApprovalRequestedEvent {
                run_id,
                request_id,
                command_segments,
                inferred_capabilities,
                blocked_rule_id,
                reason,
                reply_to_session_id,
                created_at_ms,
                ..
            }) => self.turn_scoped_record(
                "approval_requested",
                "approval",
                "info",
                run_id,
                *created_at_ms,
                &ApprovalRequestedPayload {
                    request_id: &request_id.0,
                    command_segments,
                    inferred_capabilities,
                    blocked_rule_id,
                    reason,
                    reply_to_session_id,
                },
            ),
            RuntimeEvent::ApprovalResolved(ApprovalResolvedEvent {
                run_id,
                request_id,
                action,
                created_at_ms,
                ..
            }) => self.turn_scoped_record(
                "approval_resolved",
                "approval",
                "info",
                run_id,
                *created_at_ms,
                &ApprovalResolvedPayload {
                    request_id: &request_id.0,
                    action,
                },
            ),
            RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
                run_id,
                decision,
                inferred_capabilities,
                trace_path,
                created_at_ms,
                ..
            }) => self.turn_scoped_record(
                "policy_evaluated",
                "policy",
                "info",
                run_id,
                *created_at_ms,
                &PolicyEvaluatedPayload {
                    decision,
                    inferred_capabilities,
                    trace_path,
                },
            ),
            RuntimeEvent::QuarantineCompleted(QuarantineCompletedEvent {
                run_id,
                report,
                created_at_ms,
                ..
            }) => self.turn_scoped_record(
                "quarantine_completed",
                "quarantine",
                "info",
                run_id,
                *created_at_ms,
                &QuarantineCompletedPayload { report },
            ),
            RuntimeEvent::CodexSessionStatus(CodexSessionStatusEvent {
                run_id,
                session_id,
                session_name,
                cwd,
                status,
                started_at_ms,
                updated_at_ms,
                last_step,
                summary,
                ..
            }) => self.turn_scoped_record(
                "codex_session_status",
                "codex",
                "info",
                run_id,
                *updated_at_ms,
                &CodexSessionStatusPayload {
                    session_id,
                    session_name,
                    cwd,
                    status,
                    started_at_ms: *started_at_ms,
                    updated_at_ms: *updated_at_ms,
                    last_step,
                    summary,
                },
            ),
            RuntimeEvent::AssistantMessage(AssistantMessageEvent {
                run_id,
                message,
                reply_to_session_id,
                created_at_ms,
                ..
            }) => self.turn_scoped_record(
                "assistant_message",
                "assistant",
                "info",
                run_id,
                *created_at_ms,
                &AssistantMessagePayload {
                    message,
                    reply_to_session_id,
                },
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn retained_turn_context_count(&self) -> usize {
        self.projection.retained_turn_context_count()
    }

    #[cfg(test)]
    pub(crate) fn has_retained_turn_context(&self, run_id: &RunId) -> bool {
        self.projection.has_retained_turn_context(run_id)
    }
}

#[async_trait]
impl RuntimeEventLog for FanoutRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        let value = self.runtime_event_record(&event)?;
        self.jsonl.append_json_value(&value).await?;
        self.projection.push_runtime_event(event.clone())?;
        self.event_hub.publish(&event);
        Ok(())
    }
}

pub(crate) async fn append_turn_controller_event(
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    phase: &str,
    payload: serde_json::Value,
) {
    let level = if phase.contains("error") || phase.contains("invalid") {
        "warn"
    } else {
        "info"
    };
    if let Err(err) = event_log
        .append_app_event("controller", phase, level, run_id, now_ms(), payload)
        .await
    {
        eprintln!(
            "turn controller log write failed for {} (phase={}): {}",
            run_id.0, phase, err
        );
    }
}

fn fresh_session_id() -> String {
    let uuid = Uuid::new_v4().simple().to_string();
    format!("s{:x}-{}", now_ms(), &uuid[..8])
}
