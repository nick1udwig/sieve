use async_trait::async_trait;
use serde::Serialize;
use sieve_runtime::{EventLogError, JsonlRuntimeEventLog, RuntimeEventLog};
use sieve_types::{RunId, RuntimeEvent};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

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

pub(crate) struct FanoutRuntimeEventLog {
    jsonl: JsonlRuntimeEventLog,
    history: Mutex<Vec<RuntimeEvent>>,
    telegram_tx: Mutex<Sender<TelegramLoopEvent>>,
}

impl FanoutRuntimeEventLog {
    pub(crate) fn new(
        path: impl Into<PathBuf>,
        telegram_tx: Sender<TelegramLoopEvent>,
    ) -> Result<Self, EventLogError> {
        Ok(Self {
            jsonl: JsonlRuntimeEventLog::new(path.into())?,
            history: Mutex::new(Vec::new()),
            telegram_tx: Mutex::new(telegram_tx),
        })
    }

    pub(crate) fn snapshot(&self) -> Vec<RuntimeEvent> {
        self.history
            .lock()
            .expect("runtime event history lock poisoned")
            .clone()
    }

    pub(crate) async fn append_conversation(
        &self,
        record: ConversationLogRecord,
    ) -> Result<(), EventLogError> {
        let value =
            serde_json::to_value(record).map_err(|err| EventLogError::Append(err.to_string()))?;
        self.jsonl.append_json_value(&value).await
    }
}

#[async_trait]
impl RuntimeEventLog for FanoutRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        self.jsonl.append(event.clone()).await?;
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

pub(crate) async fn append_jsonl_record(
    path: &Path,
    value: &serde_json::Value,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("jsonl path has no parent: {}", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|err| format!("failed to create jsonl parent dir: {err}"))?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|err| format!("failed to open jsonl log `{}`: {err}", path.display()))?;
    let line = format!("{value}\n");
    file.write_all(line.as_bytes())
        .await
        .map_err(|err| format!("failed to append jsonl log `{}`: {err}", path.display()))
}

pub(crate) async fn append_turn_controller_event(
    sieve_home: &Path,
    run_id: &RunId,
    phase: &str,
    payload: serde_json::Value,
) {
    let path = sieve_home.join("logs/turn-controller-events.jsonl");
    let record = serde_json::json!({
        "event": "turn_controller",
        "schema_version": 1,
        "created_at_ms": now_ms(),
        "run_id": run_id.0,
        "phase": phase,
        "payload": payload,
    });
    if let Err(err) = append_jsonl_record(&path, &record).await {
        eprintln!(
            "turn controller log write failed for {} (phase={}): {}",
            run_id.0, phase, err
        );
    }
}
