use crate::config::AppConfig;
use crate::media;
use sieve_types::{InteractionModality, ModalityContract, ModalityOverrideReason, RunId};

pub(crate) fn default_modality_contract(input: InteractionModality) -> ModalityContract {
    ModalityContract {
        input,
        response: input,
        override_reason: None,
    }
}

pub(crate) fn override_modality_contract(
    contract: &mut ModalityContract,
    response: InteractionModality,
    reason: ModalityOverrideReason,
) {
    contract.response = response;
    contract.override_reason = Some(reason);
}

pub(super) async fn resolve_trusted_user_message(
    cfg: &AppConfig,
    run_id: &RunId,
    input_modality: InteractionModality,
    media_file_id: Option<&str>,
    user_message: &str,
) -> Result<String, String> {
    match input_modality {
        InteractionModality::Text => Ok(user_message.to_string()),
        InteractionModality::Audio => match media_file_id {
            Some(file_id) => media::transcribe_audio_prompt(cfg, run_id, file_id)
                .await
                .map_err(|err| format!("audio input unavailable: {err}")),
            None => Err("audio input missing media file id".to_string()),
        },
        InteractionModality::Image => match media_file_id {
            Some(file_id) => media::extract_image_prompt(
                &cfg.telegram_bot_token,
                &cfg.sieve_home,
                run_id,
                file_id,
            )
            .await
            .map_err(|err| format!("image input unavailable: {err}")),
            None => Err("image input missing media file id".to_string()),
        },
    }
}
