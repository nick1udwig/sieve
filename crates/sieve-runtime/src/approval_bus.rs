use async_trait::async_trait;
use sieve_types::{ApprovalRequestId, ApprovalRequestedEvent, ApprovalResolvedEvent};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;
use tokio::sync::oneshot;

#[derive(Debug, Error)]
pub enum ApprovalBusError {
    #[error("approval transport failed: {0}")]
    Transport(String),
}

#[async_trait]
pub trait ApprovalBus: Send + Sync {
    async fn publish_requested(
        &self,
        event: ApprovalRequestedEvent,
    ) -> Result<(), ApprovalBusError>;

    async fn wait_resolved(
        &self,
        request_id: &ApprovalRequestId,
    ) -> Result<ApprovalResolvedEvent, ApprovalBusError>;
}

#[derive(Default)]
struct ApprovalState {
    senders: HashMap<ApprovalRequestId, oneshot::Sender<ApprovalResolvedEvent>>,
    receivers: HashMap<ApprovalRequestId, oneshot::Receiver<ApprovalResolvedEvent>>,
    published: Vec<ApprovalRequestedEvent>,
}

pub struct InProcessApprovalBus {
    state: Mutex<ApprovalState>,
}

impl InProcessApprovalBus {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ApprovalState::default()),
        }
    }

    pub fn resolve(&self, event: ApprovalResolvedEvent) -> Result<(), ApprovalBusError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApprovalBusError::Transport("approval state lock poisoned".to_string()))?;
        let Some(sender) = state.senders.remove(&event.request_id) else {
            return Err(ApprovalBusError::Transport(format!(
                "missing pending approval request: {}",
                event.request_id.0
            )));
        };
        sender
            .send(event)
            .map_err(|_| ApprovalBusError::Transport("approval receiver dropped".to_string()))
    }

    pub fn published_events(&self) -> Result<Vec<ApprovalRequestedEvent>, ApprovalBusError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ApprovalBusError::Transport("approval state lock poisoned".to_string()))?;
        Ok(state.published.clone())
    }
}

impl Default for InProcessApprovalBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ApprovalBus for InProcessApprovalBus {
    async fn publish_requested(
        &self,
        event: ApprovalRequestedEvent,
    ) -> Result<(), ApprovalBusError> {
        let (sender, receiver) = oneshot::channel();
        let mut state = self
            .state
            .lock()
            .map_err(|_| ApprovalBusError::Transport("approval state lock poisoned".to_string()))?;
        if state.senders.contains_key(&event.request_id) {
            return Err(ApprovalBusError::Transport(format!(
                "duplicate approval request id: {}",
                event.request_id.0
            )));
        }
        state.senders.insert(event.request_id.clone(), sender);
        state.receivers.insert(event.request_id.clone(), receiver);
        state.published.push(event);
        Ok(())
    }

    async fn wait_resolved(
        &self,
        request_id: &ApprovalRequestId,
    ) -> Result<ApprovalResolvedEvent, ApprovalBusError> {
        let receiver = {
            let mut state = self.state.lock().map_err(|_| {
                ApprovalBusError::Transport("approval state lock poisoned".to_string())
            })?;
            state.receivers.remove(request_id).ok_or_else(|| {
                ApprovalBusError::Transport(format!("missing approval receiver: {}", request_id.0))
            })?
        };

        receiver
            .await
            .map_err(|_| ApprovalBusError::Transport("approval sender dropped".to_string()))
    }
}
