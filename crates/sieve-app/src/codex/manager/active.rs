use super::CodexManager;
use std::future::Future;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Duration};

#[derive(Clone)]
pub(super) struct ActiveCodexTurn {
    pub(super) session_name: String,
    pub(super) steer_tx: mpsc::UnboundedSender<SteerCommand>,
}

pub(super) struct SteerCommand {
    pub(super) text: String,
    pub(super) result_tx: oneshot::Sender<Result<(), String>>,
}

impl CodexManager {
    pub(super) fn register_active_session(
        &self,
        session_id: &str,
        session_name: &str,
        steer_tx: mpsc::UnboundedSender<SteerCommand>,
    ) -> Result<(), String> {
        self.active_sessions
            .lock()
            .map_err(|_| "codex active session lock poisoned".to_string())?
            .insert(
                session_id.to_string(),
                ActiveCodexTurn {
                    session_name: session_name.to_string(),
                    steer_tx,
                },
            );
        Ok(())
    }

    pub(super) fn unregister_active_session(&self, session_id: Option<&str>) {
        let Some(session_id) = session_id else {
            return;
        };
        if let Ok(mut active) = self.active_sessions.lock() {
            active.remove(session_id);
        }
    }

    pub(super) async fn steer_session_internal(
        &self,
        session_id: &str,
        text: &str,
    ) -> Result<(), String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("codex steer text must be non-empty".to_string());
        }
        let active = self
            .active_sessions
            .lock()
            .map_err(|_| "codex active session lock poisoned".to_string())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("codex session `{session_id}` is not currently running"))?;
        let (result_tx, result_rx) = oneshot::channel();
        active
            .steer_tx
            .send(SteerCommand {
                text: trimmed.to_string(),
                result_tx,
            })
            .map_err(|_| {
                format!(
                    "codex session `{}` is no longer accepting steer input",
                    active.session_name
                )
            })?;
        result_rx.await.map_err(|_| {
            format!(
                "codex session `{}` dropped steer response channel",
                active.session_name
            )
        })?
    }

    pub(super) async fn call_with_timeout<T>(
        &self,
        label: &str,
        future: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        timeout(
            Duration::from_millis(self.config.request_timeout_ms),
            future,
        )
        .await
        .map_err(|_| {
            format!(
                "{label} timed out after {}ms",
                self.config.request_timeout_ms
            )
        })?
    }
}
