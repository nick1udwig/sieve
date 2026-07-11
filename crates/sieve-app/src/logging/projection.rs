use super::{ConversationHistoryEntry, TurnLogContext};
use sieve_runtime::EventLogError;
use sieve_types::{RunId, RuntimeEvent};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

pub(crate) const MAX_RETAINED_RUNTIME_EVENTS: usize = 512;
pub(crate) const MAX_RETAINED_CONVERSATION_ENTRIES: usize = 512;
pub(crate) const MAX_RETAINED_TURN_CONTEXTS: usize = 512;

pub(super) struct InMemoryEventProjection {
    turn_contexts: Mutex<BoundedTurnContexts>,
    runtime_events: Mutex<VecDeque<RuntimeEvent>>,
    conversation: Mutex<VecDeque<ConversationHistoryEntry>>,
}

#[derive(Default)]
struct BoundedTurnContexts {
    by_run_id: BTreeMap<RunId, TurnLogContext>,
    insertion_order: VecDeque<RunId>,
}

impl InMemoryEventProjection {
    pub(super) fn new() -> Self {
        Self {
            turn_contexts: Mutex::new(BoundedTurnContexts::default()),
            runtime_events: Mutex::new(VecDeque::new()),
            conversation: Mutex::new(VecDeque::new()),
        }
    }

    pub(super) fn insert_turn_context(&self, run_id: RunId, context: TurnLogContext) {
        self.turn_contexts
            .lock()
            .expect("turn log contexts lock poisoned")
            .insert(run_id, context);
    }

    pub(super) fn turn_context(
        &self,
        run_id: &RunId,
    ) -> Result<Option<TurnLogContext>, EventLogError> {
        self.turn_contexts
            .lock()
            .map_err(|_| EventLogError::Append("turn log contexts lock poisoned".to_string()))
            .map(|contexts| contexts.by_run_id.get(run_id).cloned())
    }

    pub(super) fn push_runtime_event(&self, event: RuntimeEvent) -> Result<(), EventLogError> {
        let mut events = self.runtime_events.lock().map_err(|_| {
            EventLogError::Append("runtime event history lock poisoned".to_string())
        })?;
        push_bounded(&mut events, event, MAX_RETAINED_RUNTIME_EVENTS);
        Ok(())
    }

    pub(super) fn runtime_snapshot(&self) -> Vec<RuntimeEvent> {
        self.runtime_events
            .lock()
            .expect("runtime event history lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub(super) fn push_conversation_entry(
        &self,
        entry: ConversationHistoryEntry,
    ) -> Result<(), EventLogError> {
        let mut conversation = self
            .conversation
            .lock()
            .map_err(|_| EventLogError::Append("conversation history lock poisoned".to_string()))?;
        push_bounded(&mut conversation, entry, MAX_RETAINED_CONVERSATION_ENTRIES);
        Ok(())
    }

    pub(super) fn conversation_snapshot(&self, session_key: &str) -> Vec<ConversationHistoryEntry> {
        self.conversation
            .lock()
            .expect("conversation history lock poisoned")
            .iter()
            .filter(|entry| entry.session_key == session_key)
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub(super) fn retained_turn_context_count(&self) -> usize {
        self.turn_contexts
            .lock()
            .expect("turn log contexts lock poisoned")
            .by_run_id
            .len()
    }

    #[cfg(test)]
    pub(super) fn has_retained_turn_context(&self, run_id: &RunId) -> bool {
        self.turn_contexts
            .lock()
            .expect("turn log contexts lock poisoned")
            .by_run_id
            .contains_key(run_id)
    }
}

impl BoundedTurnContexts {
    fn insert(&mut self, run_id: RunId, context: TurnLogContext) {
        if self.by_run_id.insert(run_id.clone(), context).is_some() {
            self.insertion_order.retain(|existing| existing != &run_id);
        }
        self.insertion_order.push_back(run_id);
        while self.by_run_id.len() > MAX_RETAINED_TURN_CONTEXTS {
            if let Some(evicted) = self.insertion_order.pop_front() {
                self.by_run_id.remove(&evicted);
            }
        }
    }
}

fn push_bounded<T>(values: &mut VecDeque<T>, value: T, limit: usize) {
    if values.len() == limit {
        values.pop_front();
    }
    values.push_back(value);
}
