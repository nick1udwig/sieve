use crate::config::AppConfig;
use crate::ingress::{IngressPrompt, PromptSource, TypingGuard};
use crate::lcm_integration::LcmIntegration;
use crate::logging::{FanoutRuntimeEventLog, TelegramLoopEvent};
use crate::turn::run_turn;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::RuntimeOrchestrator;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use tokio::sync::{mpsc as tokio_mpsc, Semaphore};

pub(crate) async fn run_agent_loop(
    runtime: Arc<RuntimeOrchestrator>,
    guidance_model: Arc<dyn GuidanceModel>,
    response_model: Arc<dyn ResponseModel>,
    summary_model: Arc<dyn SummaryModel>,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: Arc<FanoutRuntimeEventLog>,
    cfg: AppConfig,
    telegram_tx: Sender<TelegramLoopEvent>,
    mut prompt_rx: tokio_mpsc::UnboundedReceiver<IngressPrompt>,
) {
    let semaphore = Arc::new(Semaphore::new(cfg.max_concurrent_turns));
    let next_run_id = Arc::new(AtomicU64::new(1));

    eprintln!(
        "sieve-app agent mode ready; prompts accepted from stdin + Telegram chat {}",
        cfg.telegram_chat_id
    );

    while let Some(prompt) = prompt_rx.recv().await {
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };

        let runtime = runtime.clone();
        let guidance_model = guidance_model.clone();
        let response_model = response_model.clone();
        let summary_model = summary_model.clone();
        let lcm = lcm.clone();
        let event_log = event_log.clone();
        let cfg = cfg.clone();
        let telegram_tx = telegram_tx.clone();
        let source = prompt.source;
        let text = prompt.text;
        let modality = prompt.modality;
        let media_file_id = prompt.media_file_id;
        let run_index = next_run_id.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _permit = permit;
            let typing_guard = if source == PromptSource::Telegram {
                TypingGuard::start(telegram_tx, format!("run-{run_index}"))
                    .map(Some)
                    .unwrap_or(None)
            } else {
                None
            };
            if let Err(err) = run_turn(
                &runtime,
                guidance_model.as_ref(),
                response_model.as_ref(),
                summary_model.as_ref(),
                lcm.clone(),
                &event_log,
                &cfg,
                run_index,
                source,
                modality,
                media_file_id,
                text,
            )
            .await
            {
                eprintln!("run-{run_index} ({}) failed: {err}", source.as_str());
            }
            drop(typing_guard);
        });
    }
}
