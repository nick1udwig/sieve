use super::input::{
    default_modality_contract, override_modality_contract, resolve_trusted_user_message,
};
use super::planner_loop::{emit_assistant_error_message, generate_assistant_message};
use crate::config::AppConfig;
use crate::ingress::PromptSource;
use crate::lcm_integration::LcmIntegration;
use crate::logging::{now_ms, ConversationLogRecord, ConversationRole, FanoutRuntimeEventLog};
use crate::media;
use crate::personality::resolve_turn_personality;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::{RuntimeEventLog, RuntimeOrchestrator};
use sieve_types::{
    AssistantMessageEvent, InteractionModality, ModalityOverrideReason, RunId, RuntimeEvent,
};
use std::sync::Arc;

pub(crate) async fn run_turn(
    runtime: &RuntimeOrchestrator,
    guidance_model: &dyn GuidanceModel,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_index: u64,
    source: PromptSource,
    destination: Option<String>,
    input_modality: InteractionModality,
    media_file_id: Option<String>,
    user_message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_id = RunId(format!("run-{run_index}"));
    let mut modality_contract = default_modality_contract(input_modality);
    if modality_contract.response == InteractionModality::Image {
        override_modality_contract(
            &mut modality_contract,
            InteractionModality::Text,
            ModalityOverrideReason::NotSupported,
        );
    }
    let trusted_user_message = match resolve_trusted_user_message(
        cfg,
        &run_id,
        input_modality,
        media_file_id.as_deref(),
        &user_message,
    )
    .await
    {
        Ok(message) => message,
        Err(error_message) => {
            println!("{}: {}", run_id.0, error_message);
            emit_assistant_error_message(event_log, &run_id, error_message).await?;
            return Ok(());
        }
    };

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_user_message(&trusted_user_message).await {
            eprintln!("lcm ingest user failed for {}: {}", run_id.0, err);
        }
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::User,
            trusted_user_message.clone(),
            now_ms(),
        ))
        .await?;

    let personality_resolution = resolve_turn_personality(
        &cfg.sieve_home,
        source,
        destination.as_deref(),
        input_modality,
        modality_contract.response,
        &trusted_user_message,
    )?;
    let assistant_message = match personality_resolution.acknowledgement {
        Some(message) => message,
        None => {
            generate_assistant_message(
                runtime,
                guidance_model,
                response_model,
                summary_model,
                event_log,
                cfg,
                &run_id,
                &trusted_user_message,
                &personality_resolution.delivery_context,
                &personality_resolution.resolved_personality,
            )
            .await?
        }
    };
    println!("{}: {}", run_id.0, assistant_message);

    let delivered_audio = deliver_audio_reply_if_requested(
        cfg,
        source,
        destination.as_deref(),
        &run_id,
        &assistant_message,
        &mut modality_contract,
    )
    .await;

    if !delivered_audio {
        event_log
            .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
                schema_version: 1,
                run_id: run_id.clone(),
                message: assistant_message.clone(),
                created_at_ms: now_ms(),
            }))
            .await?;
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            assistant_message.clone(),
            now_ms(),
        ))
        .await?;

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_assistant_message(&assistant_message).await {
            eprintln!("lcm ingest assistant failed for {}: {}", run_id.0, err);
        }
    }
    Ok(())
}

async fn deliver_audio_reply_if_requested(
    cfg: &AppConfig,
    source: PromptSource,
    destination: Option<&str>,
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
            let chat_id = destination
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(cfg.telegram_chat_id);
            if let Err(err) =
                media::send_telegram_voice(&cfg.telegram_bot_token, chat_id, &audio_path).await
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
