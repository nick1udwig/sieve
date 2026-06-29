use crate::compose_gate::{
    combine_gate_reasons, compose_gate_followup_signal, compose_gate_requires_retry,
    extract_trusted_evidence_lines, message_negates_trusted_effects,
};
use crate::json_util::to_json_value;
use crate::logging::FanoutRuntimeEventLog;
use crate::render_refs::RenderRef;
use crate::response_style::{
    dedupe_preserve_order, denied_outcomes_only_message, enforce_link_policy,
    extract_plain_urls_from_text, obvious_meta_compose_pattern, strip_asset_urls_from_message,
    strip_unexpanded_render_tokens, user_requested_detailed_output, user_requested_sources,
};
use crate::turn::{non_empty_output_ref_ids, summarize_with_ref_id_counted};
use serde::Serialize;
use sieve_llm::{ResponseTurnInput, SummaryModel};
use sieve_types::{PlannerGuidanceSignal, RunId, TrustedToolEffect};
use std::collections::BTreeMap;
use std::path::Path;

#[path = "compose/audit.rs"]
mod audit;
#[path = "compose/effects.rs"]
mod effects;
#[path = "compose/evidence.rs"]
mod evidence;
#[path = "compose/gate.rs"]
mod gate;
#[path = "compose/sources.rs"]
mod sources;

use audit::{write_compose_audit_artifacts, ComposeAuditRequest};
use effects::trusted_effect_fallback_message;
use evidence::build_compose_evidence_summaries;
use gate::{run_compose_gate, ComposeGateRequest};
use sources::{
    collect_source_urls_from_extracted_evidence, collect_source_urls_from_refs,
    normalize_source_urls,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposePlannerDecision {
    Finalize,
    Continue(PlannerGuidanceSignal),
}

pub(crate) struct ComposeAssistantOutcome {
    pub(crate) message: String,
    pub(crate) quality_gate: Option<String>,
    pub(crate) planner_decision: ComposePlannerDecision,
    pub(crate) summary_calls: usize,
}

pub(crate) struct ComposeRequest<'a> {
    pub(crate) summary_model: &'a dyn SummaryModel,
    pub(crate) sieve_home: &'a Path,
    pub(crate) event_log: &'a FanoutRuntimeEventLog,
    pub(crate) run_id: &'a RunId,
    pub(crate) trusted_user_message: &'a str,
    pub(crate) response_input: &'a ResponseTurnInput,
    pub(crate) render_refs: &'a BTreeMap<String, RenderRef>,
    pub(crate) draft_message: String,
    pub(crate) evidence_cache: &'a mut BTreeMap<String, String>,
    pub(crate) summary_budget: usize,
}

struct ComposeContext<'a> {
    summary_model: &'a dyn SummaryModel,
    sieve_home: &'a Path,
    event_log: &'a FanoutRuntimeEventLog,
    run_id: &'a RunId,
    summary_budget: usize,
}

#[derive(Serialize)]
struct ComposeOutcomeRefPayload<'a> {
    ref_id: &'a str,
    kind: &'a str,
    byte_count: u64,
    line_count: u64,
}

#[derive(Serialize)]
struct ComposeToolOutcomePayload<'a> {
    tool_name: &'a str,
    outcome: &'a str,
    attempted_command: Option<&'a str>,
    failure_reason: Option<&'a str>,
    refs: Vec<ComposeOutcomeRefPayload<'a>>,
}

#[derive(Serialize)]
struct ComposeUserReplyPayload<'a> {
    task: &'static str,
    trusted_user_message: &'a str,
    response_modality: &'a sieve_types::InteractionModality,
    user_requested_sources: bool,
    user_requested_detailed_output: bool,
    trusted_evidence: &'a [String],
    trusted_effects: &'a [TrustedToolEffect],
    assistant_draft_message: &'a str,
    planner_thoughts: Option<&'a str>,
    tool_outcomes: &'a [ComposeToolOutcomePayload<'a>],
    extracted_evidence: &'a [sieve_llm::ResponseEvidenceRecord],
    output_ref_ids: &'a [String],
    available_plain_urls: &'a [String],
    evidence_summaries: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    compose_diagnostic: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_composed_message: Option<&'a str>,
}

fn compose_tool_outcome_payloads<'a>(
    response_input: &'a ResponseTurnInput,
) -> Vec<ComposeToolOutcomePayload<'a>> {
    response_input
        .tool_outcomes
        .iter()
        .map(|outcome| ComposeToolOutcomePayload {
            tool_name: &outcome.tool_name,
            outcome: &outcome.outcome,
            attempted_command: outcome.attempted_command.as_deref(),
            failure_reason: outcome.failure_reason.as_deref(),
            refs: outcome
                .refs
                .iter()
                .map(|ref_metadata| ComposeOutcomeRefPayload {
                    ref_id: &ref_metadata.ref_id,
                    kind: &ref_metadata.kind,
                    byte_count: ref_metadata.byte_count,
                    line_count: ref_metadata.line_count,
                })
                .collect(),
        })
        .collect()
}

pub(crate) async fn compose_assistant_message(
    request: ComposeRequest<'_>,
) -> ComposeAssistantOutcome {
    let ComposeRequest {
        summary_model,
        sieve_home,
        event_log,
        run_id,
        trusted_user_message,
        response_input,
        render_refs,
        draft_message,
        evidence_cache,
        summary_budget,
    } = request;
    let context = ComposeContext {
        summary_model,
        sieve_home,
        event_log,
        run_id,
        summary_budget,
    };
    let mut summary_calls = 0usize;
    let output_ref_ids: Vec<String> = non_empty_output_ref_ids(response_input)
        .into_iter()
        .collect();
    let mut source_urls = dedupe_preserve_order(extract_plain_urls_from_text(&draft_message));
    source_urls.extend(collect_source_urls_from_extracted_evidence(
        &response_input.extracted_evidence,
    ));
    if source_urls.is_empty() {
        source_urls.extend(collect_source_urls_from_refs(response_input, render_refs).await);
    }
    source_urls = normalize_source_urls(source_urls);
    let trusted_evidence = extract_trusted_evidence_lines(
        trusted_user_message,
        response_input.planner_thoughts.as_deref(),
    );
    let extracted_evidence = response_input.extracted_evidence.clone();
    let evidence_summaries = if extracted_evidence.is_empty() {
        build_compose_evidence_summaries(
            &context,
            trusted_user_message,
            response_input,
            render_refs,
            evidence_cache,
            &mut summary_calls,
        )
        .await
    } else {
        extracted_evidence
            .iter()
            .filter_map(|record| {
                let trimmed = record.summary.trim();
                (!trimmed.is_empty()).then_some(trimmed.to_string())
            })
            .collect()
    };
    let tool_outcomes = compose_tool_outcome_payloads(response_input);

    let mut attempt_payloads = Vec::new();
    let payload = to_json_value(
        ComposeUserReplyPayload {
            task: "compose_user_reply",
            trusted_user_message,
            response_modality: &response_input.response_modality,
            user_requested_sources: user_requested_sources(trusted_user_message),
            user_requested_detailed_output: user_requested_detailed_output(trusted_user_message),
            trusted_evidence: &trusted_evidence,
            trusted_effects: &response_input.trusted_effects,
            assistant_draft_message: &draft_message,
            planner_thoughts: response_input.planner_thoughts.as_deref(),
            tool_outcomes: &tool_outcomes,
            extracted_evidence: &extracted_evidence,
            output_ref_ids: &output_ref_ids,
            available_plain_urls: &source_urls,
            evidence_summaries: &evidence_summaries,
            compose_diagnostic: None,
            previous_composed_message: None,
        },
        "compose user reply payload",
    );
    attempt_payloads.push(payload.clone());

    let first_composed = summarize_with_ref_id_counted(
        context.summary_model,
        context.run_id,
        &format!("assistant-compose:{}", run_id.0),
        &payload,
        &mut summary_calls,
        context.summary_budget,
    )
    .await
    .unwrap_or_else(|| {
        payload
            .get("assistant_draft_message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });

    let mut composed = first_composed;
    let mut gate = run_compose_gate(ComposeGateRequest {
        context: &context,
        trusted_user_message,
        trusted_evidence: &trusted_evidence,
        trusted_effects: &response_input.trusted_effects,
        composed_message: &composed,
        extracted_evidence: &extracted_evidence,
        evidence_summaries: &evidence_summaries,
        source_urls: &source_urls,
        summary_calls: &mut summary_calls,
    })
    .await;
    let mut retry_diagnostics = Vec::new();
    if let Some(diagnostic) = compose_gate_requires_retry(
        &composed,
        trusted_user_message,
        response_input,
        gate.as_ref(),
    ) {
        retry_diagnostics.push(diagnostic);
    }
    let did_retry = !retry_diagnostics.is_empty() && summary_calls < summary_budget;
    if did_retry {
        let retry_diagnostic = retry_diagnostics.join(" | ");
        let retry_payload = to_json_value(
            ComposeUserReplyPayload {
                task: "compose_user_reply",
                trusted_user_message,
                response_modality: &response_input.response_modality,
                user_requested_sources: user_requested_sources(trusted_user_message),
                user_requested_detailed_output: user_requested_detailed_output(
                    trusted_user_message,
                ),
                trusted_evidence: &trusted_evidence,
                trusted_effects: &response_input.trusted_effects,
                assistant_draft_message: &draft_message,
                planner_thoughts: response_input.planner_thoughts.as_deref(),
                tool_outcomes: &tool_outcomes,
                extracted_evidence: &extracted_evidence,
                output_ref_ids: &output_ref_ids,
                available_plain_urls: &source_urls,
                evidence_summaries: &evidence_summaries,
                compose_diagnostic: Some(&retry_diagnostic),
                previous_composed_message: Some(&composed),
            },
            "compose retry payload",
        );
        attempt_payloads.push(retry_payload.clone());
        composed = summarize_with_ref_id_counted(
            context.summary_model,
            context.run_id,
            &format!("assistant-compose-retry:{}", run_id.0),
            &retry_payload,
            &mut summary_calls,
            context.summary_budget,
        )
        .await
        .unwrap_or_else(|| {
            retry_payload
                .get("previous_composed_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        gate = run_compose_gate(ComposeGateRequest {
            context: &context,
            trusted_user_message,
            trusted_evidence: &trusted_evidence,
            trusted_effects: &response_input.trusted_effects,
            composed_message: &composed,
            extracted_evidence: &extracted_evidence,
            evidence_summaries: &evidence_summaries,
            source_urls: &source_urls,
            summary_calls: &mut summary_calls,
        })
        .await;
    }

    let quality_gate = match gate.as_ref() {
        Some(value) if value.verdict.eq_ignore_ascii_case("PASS") => Some("PASS".to_string()),
        Some(value) => Some(format!(
            "REVISE: {}",
            value
                .reason
                .as_deref()
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or("requested revision")
        )),
        None if summary_calls >= summary_budget => {
            Some("REVISE: summary call budget exhausted".to_string())
        }
        None => Some("REVISE: missing gate verdict".to_string()),
    };
    let grounding_gate: Option<String> = None;
    let combined_gate = combine_gate_reasons(std::slice::from_ref(&quality_gate));
    let planner_followup_signal = if summary_calls >= summary_budget {
        None
    } else {
        compose_gate_followup_signal(gate.as_ref(), response_input)
    };
    let planner_decision = planner_followup_signal
        .map(ComposePlannerDecision::Continue)
        .unwrap_or(ComposePlannerDecision::Finalize);

    let mut composed = enforce_link_policy(composed, &source_urls, trusted_user_message);
    composed = strip_asset_urls_from_message(&composed);
    if let Some(message) = denied_outcomes_only_message(response_input) {
        composed = message;
    }
    if obvious_meta_compose_pattern(&composed) {
        if let Some(message) = denied_outcomes_only_message(response_input) {
            composed = message;
        } else {
            let draft_fallback = payload
                .get("assistant_draft_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !draft_fallback.is_empty() && !obvious_meta_compose_pattern(&draft_fallback) {
                composed = draft_fallback;
            }
        }
    }
    composed = strip_asset_urls_from_message(&composed);
    composed = strip_unexpanded_render_tokens(&composed);
    if message_negates_trusted_effects(&composed, response_input) {
        let draft_fallback = payload
            .get("assistant_draft_message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if !draft_fallback.is_empty()
            && !message_negates_trusted_effects(&draft_fallback, response_input)
        {
            composed = draft_fallback;
        } else if let Some(fallback) = trusted_effect_fallback_message(response_input) {
            composed = fallback;
        }
    }
    if let Err(err) = write_compose_audit_artifacts(ComposeAuditRequest {
        context: &context,
        attempts: &attempt_payloads,
        final_message: &composed,
        output_ref_ids: &output_ref_ids,
        source_urls: &source_urls,
        quality_gate: quality_gate.as_deref(),
        grounding_gate: grounding_gate.as_deref(),
        planner_followup_signal,
    })
    .await
    {
        eprintln!("compose audit write failed for {}: {}", run_id.0, err);
    }
    ComposeAssistantOutcome {
        message: composed,
        quality_gate: combined_gate,
        planner_decision,
        summary_calls,
    }
}
