use super::ComposeContext;
use crate::compose_gate::{parse_compose_gate_output, ComposeGateOutput};
use crate::json_util::to_json_value;
use crate::response_style::{user_requested_detailed_output, user_requested_sources};
use crate::turn::summarize_with_ref_id_counted;
use serde::Serialize;
use sieve_types::TrustedToolEffect;

pub(super) struct ComposeGateRequest<'a, 'b> {
    pub(super) context: &'a ComposeContext<'a>,
    pub(super) trusted_user_message: &'b str,
    pub(super) trusted_evidence: &'b [String],
    pub(super) trusted_effects: &'b [TrustedToolEffect],
    pub(super) composed_message: &'b str,
    pub(super) extracted_evidence: &'b [sieve_llm::ResponseEvidenceRecord],
    pub(super) evidence_summaries: &'b [String],
    pub(super) source_urls: &'b [String],
    pub(super) summary_calls: &'b mut usize,
}

#[derive(Serialize)]
struct ComposeGatePayload<'a> {
    task: &'static str,
    trusted_user_message: &'a str,
    user_requested_sources: bool,
    user_requested_detailed_output: bool,
    trusted_evidence: &'a [String],
    trusted_effects: &'a [TrustedToolEffect],
    composed_message: &'a str,
    extracted_evidence: &'a [sieve_llm::ResponseEvidenceRecord],
    evidence_summaries: &'a [String],
    source_urls: &'a [String],
}

pub(super) async fn run_compose_gate(
    request: ComposeGateRequest<'_, '_>,
) -> Option<ComposeGateOutput> {
    let payload = to_json_value(
        ComposeGatePayload {
            task: "compose_gate",
            trusted_user_message: request.trusted_user_message,
            user_requested_sources: user_requested_sources(request.trusted_user_message),
            user_requested_detailed_output: user_requested_detailed_output(
                request.trusted_user_message,
            ),
            trusted_evidence: request.trusted_evidence,
            trusted_effects: request.trusted_effects,
            composed_message: request.composed_message,
            extracted_evidence: request.extracted_evidence,
            evidence_summaries: request.evidence_summaries,
            source_urls: request.source_urls,
        },
        "compose gate payload",
    );
    let raw = summarize_with_ref_id_counted(
        request.context.summary_model,
        request.context.run_id,
        &format!("assistant-compose-gate:{}", request.context.run_id.0),
        &payload,
        request.summary_calls,
        request.context.summary_budget,
    )
    .await;
    parse_compose_gate_output(raw.as_deref())
}
