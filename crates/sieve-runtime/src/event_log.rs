use async_trait::async_trait;
use sieve_types::RuntimeEvent;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventLogError {
    #[error("failed to append runtime event: {0}")]
    Append(String),
}

#[async_trait]
pub trait RuntimeEventLog: Send + Sync {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError>;
}

pub struct JsonlRuntimeEventLog {
    path: PathBuf,
    writer_lock: Mutex<()>,
}

impl JsonlRuntimeEventLog {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, EventLogError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            create_dir_all(parent).map_err(|err| EventLogError::Append(err.to_string()))?;
        }
        Ok(Self {
            path,
            writer_lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn append_json_value(&self, value: &serde_json::Value) -> Result<(), EventLogError> {
        let encoded =
            serde_json::to_string(value).map_err(|err| EventLogError::Append(err.to_string()))?;
        self.append_encoded_line(&encoded)
    }

    fn append_encoded_line(&self, encoded: &str) -> Result<(), EventLogError> {
        let _guard = self
            .writer_lock
            .lock()
            .map_err(|_| EventLogError::Append("event writer lock poisoned".to_string()))?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| EventLogError::Append(err.to_string()))?;
        file.write_all(encoded.as_bytes())
            .map_err(|err| EventLogError::Append(err.to_string()))?;
        file.write_all(b"\n")
            .map_err(|err| EventLogError::Append(err.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl RuntimeEventLog for JsonlRuntimeEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        let encoded =
            serde_json::to_string(&event).map_err(|err| EventLogError::Append(err.to_string()))?;
        self.append_encoded_line(&encoded)
    }
}
