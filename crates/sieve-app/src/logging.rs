use async_trait::async_trait;
use serde::Serialize;
use sieve_runtime::{EventLogError, JsonlRuntimeEventLog, RuntimeEventLog};
use sieve_types::{
    ApprovalRequestedEvent, ApprovalResolvedEvent, AssistantMessageEvent, CodexSessionStatusEvent,
    PolicyEvaluatedEvent, QuarantineCompletedEvent, RunId, RuntimeEvent,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
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
    Runtime(RuntimeEvent),
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

pub(crate) struct FanoutRuntimeEventLog {
    jsonl: JsonlRuntimeEventLog,
    session_id: String,
    next_turn_seq: AtomicU64,
    turn_contexts: Mutex<BTreeMap<RunId, TurnLogContext>>,
    history: Mutex<Vec<RuntimeEvent>>,
    conversation_history: Mutex<Vec<ConversationHistoryEntry>>,
    telegram_tx: Mutex<Sender<TelegramLoopEvent>>,
}

impl FanoutRuntimeEventLog {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        telegram_tx: Sender<TelegramLoopEvent>,
    ) -> Result<Self, EventLogError> {
        Self::with_session_id(path, telegram_tx, fresh_session_id())
    }

    pub(crate) fn with_session_id(
        path: impl Into<PathBuf>,
        telegram_tx: Sender<TelegramLoopEvent>,
        session_id: String,
    ) -> Result<Self, EventLogError> {
        Ok(Self {
            jsonl: JsonlRuntimeEventLog::new(path.into())?,
            session_id,
            next_turn_seq: AtomicU64::new(1),
            turn_contexts: Mutex::new(BTreeMap::new()),
            history: Mutex::new(Vec::new()),
            conversation_history: Mutex::new(Vec::new()),
            telegram_tx: Mutex::new(telegram_tx),
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
        self.turn_contexts
            .lock()
            .expect("turn log contexts lock poisoned")
            .insert(
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
        self.history
            .lock()
            .expect("runtime event history lock poisoned")
            .clone()
    }

    pub(crate) fn snapshot_conversation(&self, session_key: &str) -> Vec<ConversationHistoryEntry> {
        self.conversation_history
            .lock()
            .expect("conversation history lock poisoned")
            .iter()
            .filter(|entry| entry.session_key == session_key)
            .cloned()
            .collect()
    }

    pub(crate) async fn append_conversation(
        &self,
        record: ConversationLogRecord,
    ) -> Result<(), EventLogError> {
        let payload = serde_json::json!({
            "role": record.role,
            "message": record.message,
        });
        let value = self.turn_scoped_record(
            "conversation",
            "conversation",
            "info",
            &record.run_id,
            record.created_at_ms,
            payload,
        )?;
        self.jsonl.append_json_value(&value).await?;
        let session_key = self
            .turn_contexts
            .lock()
            .map_err(|_| EventLogError::Append("turn log contexts lock poisoned".to_string()))?
            .get(&record.run_id)
            .map(|context| context.session_key.clone())
            .unwrap_or_else(|| "main".to_string());
        self.conversation_history
            .lock()
            .map_err(|_| EventLogError::Append("conversation history lock poisoned".to_string()))?
            .push(ConversationHistoryEntry {
                run_id: record.run_id,
                session_key,
                role: record.role,
                message: record.message,
                created_at_ms: record.created_at_ms,
            });
        Ok(())
    }

    pub(crate) async fn append_app_event(
        &self,
        component: &str,
        event: &str,
        level: &str,
        run_id: &RunId,
        created_at_ms: u64,
        payload: serde_json::Value,
    ) -> Result<(), EventLogError> {
        let value =
            self.turn_scoped_record(event, component, level, run_id, created_at_ms, payload)?;
        self.jsonl.append_json_value(&value).await
    }

    fn turn_scoped_record(
        &self,
        event: &str,
        component: &str,
        level: &str,
        run_id: &RunId,
        created_at_ms: u64,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, EventLogError> {
        let context = self
            .turn_contexts
            .lock()
            .map_err(|_| EventLogError::Append("turn log contexts lock poisoned".to_string()))?
            .get(run_id)
            .cloned();
        let mut record = serde_json::json!({
            "schema_version": 2,
            "event": event,
            "component": component,
            "level": level,
            "created_at_ms": created_at_ms,
            "session_id": self.session_id,
            "turn_id": run_id.0,
            "payload": payload,
        });
        if let Some(context) = context {
            record["turn_seq"] = serde_json::json!(context.turn_seq);
            record["source"] = serde_json::json!(context.source);
            record["logical_session_key"] = serde_json::json!(context.session_key);
            record["turn_kind"] = serde_json::json!(context.turn_kind);
        }
        Ok(record)
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
                serde_json::json!({
                    "request_id": request_id.0,
                    "command_segments": command_segments,
                    "inferred_capabilities": inferred_capabilities,
                    "blocked_rule_id": blocked_rule_id,
                    "reason": reason,
                    "reply_to_session_id": reply_to_session_id,
                }),
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
                serde_json::json!({
                    "request_id": request_id.0,
                    "action": action,
                }),
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
                serde_json::json!({
                    "decision": decision,
                    "inferred_capabilities": inferred_capabilities,
                    "trace_path": trace_path,
                }),
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
                serde_json::json!({
                    "report": report,
                }),
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
                serde_json::json!({
                    "session_id": session_id,
                    "session_name": session_name,
                    "cwd": cwd,
                    "status": status,
                    "started_at_ms": started_at_ms,
                    "updated_at_ms": updated_at_ms,
                    "last_step": last_step,
                    "summary": summary,
                }),
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
                serde_json::json!({
                    "message": message,
                    "reply_to_session_id": reply_to_session_id,
                }),
            ),
        }
    }
}

#[async_trait]
impl RuntimeEventLog for FanoutRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        let value = self.runtime_event_record(&event)?;
        self.jsonl.append_json_value(&value).await?;
        self.history
            .lock()
            .map_err(|_| EventLogError::Append("runtime event history lock poisoned".to_string()))?
            .push(event.clone());
        self.telegram_tx
            .lock()
            .map_err(|_| EventLogError::Append("telegram event sender lock poisoned".to_string()))?
            .send(TelegramLoopEvent::Runtime(event))
            .map_err(|err| {
                EventLogError::Append(format!("failed to forward runtime event: {err}"))
            })?;
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
