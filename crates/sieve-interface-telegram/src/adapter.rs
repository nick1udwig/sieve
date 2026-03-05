use crate::{
    message::{
        format_approval_requested, parse_command, parse_reaction_action, parse_short_action,
        TelegramApprovalAction,
    },
    Clock, TelegramAdapterConfig, TelegramAdapterError, TelegramEventBridge, TelegramLongPoll,
    TelegramPrompt, TelegramUpdate, TELEGRAM_IMAGE_PROMPT_PREFIX, TELEGRAM_VOICE_PROMPT_PREFIX,
};
use sieve_types::{
    ApprovalAction, ApprovalRequestedEvent, ApprovalResolvedEvent, InteractionModality,
    RuntimeEvent,
};
use std::collections::{BTreeMap, BTreeSet};

pub struct TelegramAdapter<B, P, C>
where
    B: TelegramEventBridge,
    P: TelegramLongPoll,
    C: Clock,
{
    config: TelegramAdapterConfig,
    bridge: B,
    poll: P,
    clock: C,
    next_update_offset: Option<i64>,
    pending_approvals: BTreeMap<String, ApprovalRequestedEvent>,
    pending_approval_message_ids: BTreeMap<i64, String>,
    active_typing_runs: BTreeSet<String>,
    last_typing_sent_ms: Option<u64>,
}

impl<B, P, C> TelegramAdapter<B, P, C>
where
    B: TelegramEventBridge,
    P: TelegramLongPoll,
    C: Clock,
{
    pub fn new(config: TelegramAdapterConfig, bridge: B, poll: P, clock: C) -> Self {
        Self {
            config,
            bridge,
            poll,
            clock,
            next_update_offset: None,
            pending_approvals: BTreeMap::new(),
            pending_approval_message_ids: BTreeMap::new(),
            active_typing_runs: BTreeSet::new(),
            last_typing_sent_ms: None,
        }
    }

    const TYPING_REFRESH_MS: u64 = 4_000;

    pub fn start_typing(&mut self, run_id: impl Into<String>) -> Result<(), TelegramAdapterError> {
        self.active_typing_runs.insert(run_id.into());
        self.last_typing_sent_ms = None;
        self.refresh_typing()?;
        Ok(())
    }

    pub fn stop_typing(&mut self, run_id: &str) {
        self.active_typing_runs.remove(run_id);
    }

    pub fn publish_runtime_event(
        &mut self,
        event: RuntimeEvent,
    ) -> Result<(), TelegramAdapterError> {
        self.bridge.publish_runtime_event(&event);

        match event {
            RuntimeEvent::ApprovalRequested(event) => {
                let key = event.request_id.0.clone();
                let text = format_approval_requested(&event);
                self.pending_approvals.insert(key.clone(), event);
                if let Some(message_id) = self.send_to_chat(&text)? {
                    self.pending_approval_message_ids.insert(message_id, key);
                }
            }
            RuntimeEvent::PolicyEvaluated(_) => {}
            RuntimeEvent::QuarantineCompleted(_) => {}
            RuntimeEvent::AssistantMessage(event) => {
                self.stop_typing(&event.run_id.0);
                self.send_to_chat(&event.message)?;
            }
            RuntimeEvent::ApprovalResolved(event) => {
                self.pending_approvals.remove(&event.request_id.0);
                self.pending_approval_message_ids
                    .retain(|_, request_id| request_id != &event.request_id.0);
            }
        }

        Ok(())
    }

    pub fn poll_once(&mut self) -> Result<(), TelegramAdapterError> {
        self.refresh_typing()?;
        let timeout_secs = if self.active_typing_runs.is_empty() {
            self.config.poll_timeout_secs
        } else {
            self.config.poll_timeout_secs.min(1)
        };
        let updates = self
            .poll
            .get_updates(self.next_update_offset, timeout_secs)
            .map_err(TelegramAdapterError::Transport)?;

        for update in updates {
            self.next_update_offset = Some(update.update_id + 1);
            self.handle_update(update)?;
        }

        Ok(())
    }

    pub fn run_long_poll_loop(&mut self) -> Result<(), TelegramAdapterError> {
        loop {
            self.poll_once()?;
        }
    }

    fn handle_update(&mut self, update: TelegramUpdate) -> Result<(), TelegramAdapterError> {
        if let Some(message) = update.message {
            if message.chat_id != self.config.chat_id {
                return Ok(());
            }
            if !self.sender_allowed(message.sender_user_id) {
                return Ok(());
            }

            if let Some(file_id) = message
                .text
                .trim()
                .strip_prefix(TELEGRAM_VOICE_PROMPT_PREFIX)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                self.bridge.submit_prompt(TelegramPrompt {
                    chat_id: message.chat_id,
                    text: String::new(),
                    modality: InteractionModality::Audio,
                    media_file_id: Some(file_id.to_string()),
                });
                return Ok(());
            }
            if let Some(file_id) = message
                .text
                .trim()
                .strip_prefix(TELEGRAM_IMAGE_PROMPT_PREFIX)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                self.bridge.submit_prompt(TelegramPrompt {
                    chat_id: message.chat_id,
                    text: String::new(),
                    modality: InteractionModality::Image,
                    media_file_id: Some(file_id.to_string()),
                });
                return Ok(());
            }

            if let Some(command) = parse_command(&message.text) {
                self.resolve_approval(command.action, command.request_id)?;
                return Ok(());
            }

            if let Some(action) = parse_short_action(&message.text) {
                if self.pending_approvals.is_empty() {
                    self.bridge.submit_prompt(TelegramPrompt {
                        chat_id: message.chat_id,
                        text: message.text,
                        modality: InteractionModality::Text,
                        media_file_id: None,
                    });
                    return Ok(());
                }
                if let Some(request_id) =
                    self.select_request_for_implicit_action(message.reply_to_message_id)
                {
                    self.resolve_approval(action, request_id)?;
                } else {
                    self.send_to_chat(
                        "approval target unclear; reply to an approval request message or use /approve_once <request_id>, /always <request_id>, or /deny <request_id>",
                    )?;
                }
                return Ok(());
            }

            self.bridge.submit_prompt(TelegramPrompt {
                chat_id: message.chat_id,
                text: message.text,
                modality: InteractionModality::Text,
                media_file_id: None,
            });
            return Ok(());
        }

        if let Some(reaction) = update.message_reaction {
            if reaction.chat_id != self.config.chat_id {
                return Ok(());
            }
            if !self.sender_allowed(reaction.sender_user_id) {
                return Ok(());
            }
            let Some(action) = parse_reaction_action(&reaction.emoji) else {
                return Ok(());
            };
            let Some(request_id) = self
                .pending_approval_message_ids
                .get(&reaction.message_id)
                .cloned()
            else {
                return Ok(());
            };
            self.resolve_approval(action, request_id)?;
        }

        Ok(())
    }

    fn sender_allowed(&self, sender_user_id: Option<i64>) -> bool {
        match &self.config.allowed_sender_user_ids {
            Some(allowed_sender_user_ids) => sender_user_id
                .map(|user_id| allowed_sender_user_ids.contains(&user_id))
                .unwrap_or(false),
            None => true,
        }
    }

    fn select_request_for_implicit_action(
        &self,
        reply_to_message_id: Option<i64>,
    ) -> Option<String> {
        if let Some(reply_id) = reply_to_message_id {
            if let Some(request_id) = self.pending_approval_message_ids.get(&reply_id) {
                return Some(request_id.clone());
            }
        }
        if self.pending_approvals.len() == 1 {
            return self.pending_approvals.keys().next().cloned();
        }
        None
    }

    fn resolve_approval(
        &mut self,
        action: TelegramApprovalAction,
        request_id: String,
    ) -> Result<(), TelegramAdapterError> {
        let Some(approval_requested) = self.pending_approvals.remove(&request_id) else {
            return Ok(());
        };
        self.pending_approval_message_ids
            .retain(|_, mapped_request_id| mapped_request_id != &request_id);

        let action = match action {
            TelegramApprovalAction::ApproveOnce => ApprovalAction::ApproveOnce,
            TelegramApprovalAction::ApproveAlways => ApprovalAction::ApproveAlways,
            TelegramApprovalAction::Deny => ApprovalAction::Deny,
        };

        let resolved = ApprovalResolvedEvent {
            schema_version: approval_requested.schema_version,
            request_id: approval_requested.request_id,
            run_id: approval_requested.run_id,
            action,
            created_at_ms: self.clock.now_ms(),
        };
        self.bridge.submit_approval(resolved.clone());
        Ok(())
    }

    fn send_to_chat(&mut self, text: &str) -> Result<Option<i64>, TelegramAdapterError> {
        self.poll
            .send_message(self.config.chat_id, text)
            .map_err(TelegramAdapterError::Transport)
    }

    fn refresh_typing(&mut self) -> Result<(), TelegramAdapterError> {
        if self.active_typing_runs.is_empty() {
            return Ok(());
        }
        let now = self.clock.now_ms();
        let due = self
            .last_typing_sent_ms
            .map(|last| now.saturating_sub(last) >= Self::TYPING_REFRESH_MS)
            .unwrap_or(true);
        if !due {
            return Ok(());
        }

        self.poll
            .send_chat_action(self.config.chat_id, "typing")
            .map_err(TelegramAdapterError::Transport)?;
        self.last_typing_sent_ms = Some(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
