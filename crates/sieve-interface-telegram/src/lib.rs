#![forbid(unsafe_code)]

use sieve_types::{ApprovalResolvedEvent, RuntimeEvent, UnixMillis};
use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

mod adapter;
mod message;
mod transport;

pub use adapter::TelegramAdapter;
pub use transport::TelegramBotApiLongPoll;

pub struct TelegramAdapterConfig {
    pub chat_id: i64,
    pub poll_timeout_secs: u16,
    pub allowed_sender_user_ids: Option<BTreeSet<i64>>,
}

impl Default for TelegramAdapterConfig {
    fn default() -> Self {
        Self {
            chat_id: 0,
            poll_timeout_secs: 30,
            allowed_sender_user_ids: None,
        }
    }
}

pub trait TelegramEventBridge: Send + Sync {
    fn publish_runtime_event(&self, event: &RuntimeEvent);

    fn submit_approval(&self, approval: ApprovalResolvedEvent);

    fn submit_prompt(&self, _prompt: TelegramPrompt) {}
}

pub trait TelegramLongPoll: Send {
    fn get_updates(
        &mut self,
        offset: Option<i64>,
        timeout_secs: u16,
    ) -> Result<Vec<TelegramUpdate>, String>;

    fn send_message(&mut self, chat_id: i64, text: &str) -> Result<Option<i64>, String>;
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> UnixMillis;
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> UnixMillis {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch");
        now.as_millis() as UnixMillis
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramMessage {
    pub chat_id: i64,
    pub sender_user_id: Option<i64>,
    pub message_id: i64,
    pub reply_to_message_id: Option<i64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub message_reaction: Option<TelegramMessageReaction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramMessageReaction {
    pub chat_id: i64,
    pub sender_user_id: Option<i64>,
    pub message_id: i64,
    pub emoji: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramPrompt {
    pub chat_id: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramAdapterError {
    Transport(String),
}
