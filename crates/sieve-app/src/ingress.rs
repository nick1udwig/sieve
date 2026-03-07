use crate::config::AppConfig;
use crate::logging::TelegramLoopEvent;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge, TelegramPrompt,
};
use sieve_runtime::{ApprovalBusError, InProcessApprovalBus};
use sieve_types::{ApprovalResolvedEvent, InteractionModality, RuntimeEvent};
use std::io::{self, BufRead};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptSource {
    Stdin,
    Telegram,
}

impl PromptSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PromptSource::Stdin => "stdin",
            PromptSource::Telegram => "telegram",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IngressPrompt {
    pub(crate) source: PromptSource,
    pub(crate) destination: Option<String>,
    pub(crate) text: String,
    pub(crate) modality: InteractionModality,
    pub(crate) media_file_id: Option<String>,
}

pub(crate) struct RuntimeBridge {
    approval_bus: Arc<InProcessApprovalBus>,
    prompt_tx: Option<tokio_mpsc::UnboundedSender<IngressPrompt>>,
}

impl RuntimeBridge {
    pub(crate) fn new(approval_bus: Arc<InProcessApprovalBus>) -> Self {
        Self {
            approval_bus,
            prompt_tx: None,
        }
    }

    pub(crate) fn with_prompt_tx(
        approval_bus: Arc<InProcessApprovalBus>,
        prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
    ) -> Self {
        Self {
            approval_bus,
            prompt_tx: Some(prompt_tx),
        }
    }
}

impl TelegramEventBridge for RuntimeBridge {
    fn publish_runtime_event(&self, _event: &RuntimeEvent) {}

    fn submit_approval(&self, approval: ApprovalResolvedEvent) {
        if let Err(err) = self.approval_bus.resolve(approval) {
            eprintln!(
                "telegram bridge failed to resolve approval: {}",
                format_approval_bus_error(&err)
            );
        }
    }

    fn submit_prompt(&self, prompt: TelegramPrompt) {
        let text = prompt.text.trim().to_string();
        if prompt.modality == InteractionModality::Text && text.is_empty() {
            return;
        }
        if let Some(prompt_tx) = &self.prompt_tx {
            if let Err(err) = prompt_tx.send(IngressPrompt {
                source: PromptSource::Telegram,
                destination: Some(prompt.chat_id.to_string()),
                text,
                modality: prompt.modality,
                media_file_id: prompt.media_file_id,
            }) {
                eprintln!("failed to enqueue telegram prompt: {err}");
            }
        }
    }
}

fn format_approval_bus_error(err: &ApprovalBusError) -> String {
    err.to_string()
}

pub(crate) fn spawn_telegram_loop(
    cfg: &AppConfig,
    bridge: RuntimeBridge,
    event_rx: Receiver<TelegramLoopEvent>,
) -> thread::JoinHandle<()> {
    let bot_token = cfg.telegram_bot_token.clone();
    let chat_id = cfg.telegram_chat_id;
    let poll_timeout_secs = cfg.telegram_poll_timeout_secs;
    let allowed_sender_user_ids = cfg.telegram_allowed_sender_user_ids.clone();

    thread::spawn(move || {
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id,
                poll_timeout_secs,
                allowed_sender_user_ids,
            },
            bridge,
            TelegramBotApiLongPoll::new(bot_token),
            TelegramClock,
        );

        loop {
            let mut disconnected = false;
            loop {
                match event_rx.try_recv() {
                    Ok(TelegramLoopEvent::Runtime(event)) => {
                        if let Err(err) = adapter.publish_runtime_event(event) {
                            eprintln!("telegram publish runtime event failed: {err:?}");
                        }
                    }
                    Ok(TelegramLoopEvent::TypingStart { run_id }) => {
                        if let Err(err) = adapter.start_typing(run_id) {
                            eprintln!("telegram typing start failed: {err:?}");
                        }
                    }
                    Ok(TelegramLoopEvent::TypingStop { run_id }) => {
                        adapter.stop_typing(&run_id);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if disconnected {
                break;
            }

            if let Err(err) = adapter.poll_once() {
                eprintln!("telegram poll failed: {err:?}");
                thread::sleep(Duration::from_secs(1));
            }
        }
    })
}

pub(crate) fn spawn_stdin_prompt_loop(
    prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    let prompt = line.trim();
                    if prompt.is_empty() {
                        continue;
                    }
                    if let Err(err) = prompt_tx.send(IngressPrompt {
                        source: PromptSource::Stdin,
                        destination: None,
                        text: prompt.to_string(),
                        modality: InteractionModality::Text,
                        media_file_id: None,
                    }) {
                        eprintln!("stdin prompt loop stopped: {err}");
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("stdin read failed: {err}");
                    break;
                }
            }
        }
    })
}

pub(crate) struct TypingGuard {
    telegram_tx: Sender<TelegramLoopEvent>,
    run_id: String,
}

impl TypingGuard {
    pub(crate) fn start(
        telegram_tx: Sender<TelegramLoopEvent>,
        run_id: String,
    ) -> Result<Self, mpsc::SendError<TelegramLoopEvent>> {
        telegram_tx.send(TelegramLoopEvent::TypingStart {
            run_id: run_id.clone(),
        })?;
        Ok(Self {
            telegram_tx,
            run_id,
        })
    }
}

impl Drop for TypingGuard {
    fn drop(&mut self) {
        let _ = self.telegram_tx.send(TelegramLoopEvent::TypingStop {
            run_id: self.run_id.clone(),
        });
    }
}
