use super::super::response_refs::{
    build_response_evidence_records, build_response_turn_input, non_empty_output_ref_ids,
    requires_output_visibility, response_has_visible_selected_output,
};
use crate::render_refs::render_assistant_message;
use crate::render_refs::RenderRef;
use crate::response_style::strip_unexpanded_render_tokens;
use sieve_llm::{LlmError, ResponseModel, ResponseTurnInput, SummaryModel};
use sieve_runtime::PlannerRunResult;
use sieve_types::{InteractionModality, RunId};
use std::collections::BTreeMap;

pub(super) struct WriteResponseDraftRequest<'a> {
    pub(super) response_model: &'a dyn ResponseModel,
    pub(super) summary_model: &'a dyn SummaryModel,
    pub(super) run_id: &'a RunId,
    pub(super) trusted_user_message: &'a str,
    pub(super) response_modality: InteractionModality,
    pub(super) aggregated_result: &'a PlannerRunResult,
    pub(super) summary_calls_used: &'a mut usize,
    pub(super) summary_call_budget: usize,
}

pub(super) struct ResponseDraft {
    pub(super) response_input: ResponseTurnInput,
    pub(super) render_refs: BTreeMap<String, RenderRef>,
    pub(super) draft_message: String,
}

pub(super) async fn write_response_draft(
    request: WriteResponseDraftRequest<'_>,
) -> Result<ResponseDraft, LlmError> {
    let (mut response_input, render_refs) = build_response_turn_input(
        request.run_id,
        request.trusted_user_message,
        request.response_modality,
        request.aggregated_result,
    );
    response_input.extracted_evidence = build_response_evidence_records(
        request.summary_model,
        request.run_id,
        request.trusted_user_message,
        &response_input,
        &render_refs,
        request.summary_calls_used,
        request.summary_call_budget,
    )
    .await;

    let mut response_output = request
        .response_model
        .write_turn_response(response_input.clone())
        .await?;

    if requires_output_visibility(&response_input)
        && !response_has_visible_selected_output(&response_input, &response_output)
    {
        let diagnostics = "Non-empty output refs exist (stdout/stderr). Include at least one output token directly in `message` using [[ref:<id>]] or [[summary:<id>]], and list the same id in referenced_ref_ids or summarized_ref_ids.";
        response_input.planner_thoughts = Some(match response_input.planner_thoughts.take() {
            Some(existing) if !existing.trim().is_empty() => {
                format!("{existing}\n{diagnostics}")
            }
            _ => diagnostics.to_string(),
        });

        response_output = request
            .response_model
            .write_turn_response(response_input.clone())
            .await?;

        if !response_has_visible_selected_output(&response_input, &response_output) {
            if let Some(fallback_ref_id) =
                non_empty_output_ref_ids(&response_input).into_iter().next()
            {
                response_output
                    .summarized_ref_ids
                    .insert(fallback_ref_id.clone());
                let token = format!("[[summary:{fallback_ref_id}]]");
                if !response_output.message.contains(&token) {
                    let base = response_output.message.trim();
                    response_output.message = if base.is_empty() {
                        token
                    } else {
                        format!("{base}\n{token}")
                    };
                }
            }
        }
    }

    let draft_message = if requires_output_visibility(&response_input) {
        render_assistant_message(
            &response_output.message,
            &response_output.referenced_ref_ids,
            &response_output.summarized_ref_ids,
            &render_refs,
            request.summary_model,
            request.run_id,
        )
        .await
    } else {
        let stripped = strip_unexpanded_render_tokens(&response_output.message);
        if stripped.trim().is_empty() {
            render_assistant_message(
                &response_output.message,
                &response_output.referenced_ref_ids,
                &response_output.summarized_ref_ids,
                &render_refs,
                request.summary_model,
                request.run_id,
            )
            .await
        } else {
            stripped
        }
    };

    Ok(ResponseDraft {
        response_input,
        render_refs,
        draft_message,
    })
}
