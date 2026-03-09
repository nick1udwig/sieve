use super::input::{
    default_modality_contract, override_modality_contract, resolve_trusted_user_message,
};
use super::planner_loop::{
    emit_assistant_error_message, generate_assistant_message, GeneratedAssistantMessage,
};
use crate::config::AppConfig;
use crate::ingress::{IngressPrompt, PromptSource};
use crate::lcm_integration::LcmIntegration;
use crate::logging::{now_ms, ConversationLogRecord, ConversationRole, FanoutRuntimeEventLog};
use crate::media;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::{RuntimeEventLog, RuntimeOrchestrator};
use sieve_types::{
    AssistantMessageEvent, InteractionModality, ModalityOverrideReason, RunId, RuntimeEvent,
};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnOutcome {
    pub(crate) trusted_user_message: String,
    pub(crate) assistant_message: String,
    pub(crate) assistant_delivered: bool,
    pub(crate) assistant_suppressed_heartbeat_ok: bool,
}

pub(crate) async fn run_turn(
    runtime: &RuntimeOrchestrator,
    guidance_model: &dyn GuidanceModel,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_id: RunId,
    prompt: &IngressPrompt,
) -> Result<TurnOutcome, Box<dyn std::error::Error>> {
    let mut modality_contract = default_modality_contract(prompt.modality);
    if modality_contract.response == InteractionModality::Image {
        override_modality_contract(
            &mut modality_contract,
            InteractionModality::Text,
            ModalityOverrideReason::NotSupported,
        );
    }
    let trusted_user_message = match resolve_trusted_user_message(
        runtime,
        cfg,
        &run_id,
        prompt.modality,
        prompt.media_file_id.as_deref(),
        &prompt.text,
    )
    .await
    {
        Ok(message) => message,
        Err(error_message) => {
            println!("{}: {}", run_id.0, error_message);
            emit_assistant_error_message(event_log, &run_id, error_message).await?;
            return Ok(TurnOutcome {
                trusted_user_message: String::new(),
                assistant_message: "error".to_string(),
                assistant_delivered: true,
                assistant_suppressed_heartbeat_ok: false,
            });
        }
    };

    if let Some(memory) = lcm.as_ref() {
        if prompt.turn_kind.ingests_user_message() {
            if let Err(err) = memory
                .ingest_user_message_for_session(&prompt.session_key, &trusted_user_message)
                .await
            {
                eprintln!("lcm ingest user failed for {}: {}", run_id.0, err);
            }
        }
    }

    if prompt.turn_kind.logs_user_conversation() {
        event_log
            .append_conversation(ConversationLogRecord::new(
                run_id.clone(),
                ConversationRole::User,
                trusted_user_message.clone(),
                now_ms(),
            ))
            .await?;
    }

    let assistant_message = generate_assistant_message(
        runtime,
        guidance_model,
        response_model,
        summary_model,
        event_log,
        cfg,
        &run_id,
        &trusted_user_message,
        modality_contract.response,
        &prompt.turn_kind,
    )
    .await?;

    let (assistant_message, assistant_delivered, assistant_suppressed_heartbeat_ok) =
        match assistant_message {
            GeneratedAssistantMessage::Deliver(message) => (message, true, false),
            GeneratedAssistantMessage::SuppressHeartbeat => (String::new(), false, true),
        };
    let delivered_text = assistant_message.as_str();
    if assistant_delivered {
        println!("{}: {}", run_id.0, delivered_text);
    }

    let delivered_audio = deliver_audio_reply_if_requested(
        cfg,
        prompt.source,
        &run_id,
        delivered_text,
        &mut modality_contract,
    )
    .await;

    if assistant_delivered && !delivered_audio {
        event_log
            .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
                schema_version: 1,
                run_id: run_id.clone(),
                message: delivered_text.to_string(),
                created_at_ms: now_ms(),
            }))
            .await?;
    }

    if prompt
        .turn_kind
        .logs_assistant_conversation(assistant_delivered || delivered_audio)
    {
        event_log
            .append_conversation(ConversationLogRecord::new(
                run_id.clone(),
                ConversationRole::Assistant,
                delivered_text.to_string(),
                now_ms(),
            ))
            .await?;
    }

    if let Some(memory) = lcm.as_ref() {
        if prompt
            .turn_kind
            .ingests_assistant_message(assistant_delivered || delivered_audio)
        {
            if let Err(err) = memory
                .ingest_assistant_message_for_session(&prompt.session_key, delivered_text)
                .await
            {
                eprintln!("lcm ingest assistant failed for {}: {}", run_id.0, err);
            }
        }
    }
    Ok(TurnOutcome {
        trusted_user_message,
        assistant_message,
        assistant_delivered: assistant_delivered || delivered_audio,
        assistant_suppressed_heartbeat_ok,
    })
}

async fn deliver_audio_reply_if_requested(
    cfg: &AppConfig,
    source: PromptSource,
    run_id: &RunId,
    assistant_message: &str,
    modality_contract: &mut sieve_types::ModalityContract,
) -> bool {
    if source != PromptSource::Telegram || modality_contract.response != InteractionModality::Audio
    {
        return false;
    }

    match media::synthesize_audio_reply(cfg, run_id, assistant_message).await {
        Ok(audio_path) => {
            if let Err(err) = media::send_telegram_voice(
                &cfg.telegram_bot_token,
                cfg.telegram_chat_id,
                &audio_path,
            )
            .await
            {
                eprintln!("audio reply delivery failed for {}: {}", run_id.0, err);
                override_modality_contract(
                    modality_contract,
                    InteractionModality::Text,
                    ModalityOverrideReason::ToolFailure,
                );
                false
            } else {
                true
            }
        }
        Err(err) => {
            eprintln!("audio synthesis failed for {}: {}", run_id.0, err);
            override_modality_contract(
                modality_contract,
                InteractionModality::Text,
                ModalityOverrideReason::ToolFailure,
            );
            false
        }
    }
}
