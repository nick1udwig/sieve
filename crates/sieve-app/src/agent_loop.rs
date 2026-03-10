use crate::automation::AutomationManager;
use crate::config::AppConfig;
use crate::ingress::{IngressPrompt, PromptSource, TypingGuard};
use crate::lcm_integration::LcmIntegration;
use crate::logging::{
    now_ms, ConversationLogRecord, ConversationRole, FanoutRuntimeEventLog, TelegramLoopEvent,
};
use crate::turn::run_turn;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::{RuntimeEventLog, RuntimeOrchestrator};
use sieve_types::{AssistantMessageEvent, RuntimeEvent};
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
    automation: Option<Arc<AutomationManager>>,
    telegram_tx: Sender<TelegramLoopEvent>,
    mut prompt_rx: tokio_mpsc::UnboundedReceiver<IngressPrompt>,
) {
    let semaphore = Arc::new(Semaphore::new(cfg.max_concurrent_turns));

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
        let automation = automation.clone();
        let telegram_tx = telegram_tx.clone();
        let reserved_turn = event_log.reserve_turn_with_metadata(
            prompt.source.as_str(),
            &prompt.session_key,
            prompt.turn_kind.as_str(),
        );
        let source = prompt.source;
        let turn_prompt = prompt;

        tokio::spawn(async move {
            let _permit = permit;
            let typing_guard = if source == PromptSource::Telegram {
                TypingGuard::start(telegram_tx, reserved_turn.run_id.0.clone())
                    .map(Some)
                    .unwrap_or(None)
            } else {
                None
            };
            if let Some(automation) = automation.as_ref() {
                if matches!(turn_prompt.turn_kind, crate::ingress::TurnKind::User) {
                    match automation.handle_command(&turn_prompt.text).await {
                        Ok(Some(reply)) => {
                            if let Err(err) = emit_direct_assistant_message(
                                &event_log,
                                &reserved_turn.run_id,
                                &reply,
                            )
                            .await
                            {
                                eprintln!(
                                    "{} ({}) direct command reply failed: {err}",
                                    reserved_turn.run_id.0,
                                    source.as_str()
                                );
                            }
                            drop(typing_guard);
                            return;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            if let Err(log_err) = emit_direct_assistant_message(
                                &event_log,
                                &reserved_turn.run_id,
                                &format!("error: {err}"),
                            )
                            .await
                            {
                                eprintln!(
                                    "{} ({}) direct command error reply failed: {log_err}",
                                    reserved_turn.run_id.0,
                                    source.as_str()
                                );
                            }
                            drop(typing_guard);
                            return;
                        }
                    }
                }
                automation.note_turn_started(&turn_prompt);
            }

            let (outcome_for_automation, error_for_automation) = match run_turn(
                &runtime,
                guidance_model.as_ref(),
                response_model.as_ref(),
                summary_model.as_ref(),
                lcm.clone(),
                &event_log,
                &cfg,
                reserved_turn.run_id.clone(),
                &turn_prompt,
            )
            .await
            {
                Ok(outcome) => (Some(outcome), None),
                Err(err) => (None, Some(err.to_string())),
            };

            if let Some(automation) = automation.as_ref() {
                automation
                    .note_turn_finished(
                        &turn_prompt,
                        outcome_for_automation.as_ref(),
                        error_for_automation.clone(),
                    )
                    .await;
            }

            if let Some(err) = error_for_automation {
                eprintln!(
                    "{} ({}) failed: {err}",
                    reserved_turn.run_id.0,
                    source.as_str()
                );
            }
            drop(typing_guard);
        });
    }
}

async fn emit_direct_assistant_message(
    event_log: &FanoutRuntimeEventLog,
    run_id: &sieve_types::RunId,
    message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}: {}", run_id.0, message);
    event_log
        .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: run_id.clone(),
            message: message.to_string(),
            reply_to_session_id: None,
            created_at_ms: now_ms(),
        }))
        .await?;
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            message.to_string(),
            now_ms(),
        ))
        .await?;
    Ok(())
}
