#![forbid(unsafe_code)]

use sieve_types::{ApprovalResolvedEvent, RuntimeEvent, UnixMillis};
use std::time::{SystemTime, UNIX_EPOCH};

mod adapter;
mod message;
mod transport;

pub use adapter::TelegramAdapter;
pub use transport::TelegramBotApiLongPoll;

pub struct TelegramAdapterConfig {
    pub chat_id: i64,
    pub poll_timeout_secs: u16,
}

impl Default for TelegramAdapterConfig {
    fn default() -> Self {
        Self {
            chat_id: 0,
            poll_timeout_secs: 30,
        }
    }
}

pub trait TelegramEventBridge: Send + Sync {
    fn publish_runtime_event(&self, event: &RuntimeEvent);

    fn submit_approval(&self, approval: ApprovalResolvedEvent);
}

pub trait TelegramLongPoll: Send {
    fn get_updates(
        &mut self,
        offset: Option<i64>,
        timeout_secs: u16,
    ) -> Result<Vec<TelegramUpdate>, String>;

    fn send_message(&mut self, chat_id: i64, text: &str) -> Result<(), String>;
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
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelegramAdapterError {
    Transport(String),
}
