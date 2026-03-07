use crate::compose_gate::{
    combine_gate_reasons, compose_gate_followup_signal, compose_gate_requires_retry,
    extract_trusted_evidence_lines, parse_compose_gate_output, ComposeGateOutput,
};
use crate::logging::{now_ms, FanoutRuntimeEventLog};
use crate::render_refs::{resolve_ref_summary_input, RenderRef};
use crate::response_style::{
    dedupe_preserve_order, denied_outcomes_only_message, enforce_link_policy,
    extract_plain_urls_from_text, filter_non_asset_urls, obvious_meta_compose_pattern,
    strip_asset_urls_from_message, strip_unexpanded_render_tokens, user_requested_detailed_output,
    user_requested_sources,
};
use crate::turn::{non_empty_output_ref_ids, summarize_with_ref_id_counted};
use sieve_llm::{ResponseTurnInput, SummaryModel};
use sieve_types::{PlannerGuidanceSignal, RunId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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

async fn write_compose_audit_artifacts(
    sieve_home: &Path,
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    attempts: &[serde_json::Value],
    final_message: &str,
    output_ref_ids: &[String],
    source_urls: &[String],
    quality_gate: Option<&str>,
    grounding_gate: Option<&str>,
    planner_followup_signal: Option<PlannerGuidanceSignal>,
) -> Result<(), String> {
    let run_dir = sieve_home.join("artifacts").join(&run_id.0);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|err| format!("failed to create compose artifact dir: {err}"))?;

    let mut input_refs = Vec::new();
    for (idx, attempt) in attempts.iter().enumerate() {
        let ref_id = format!("assistant-compose-input:{}:{}", run_id.0, idx + 1);
        let path = run_dir.join(format!("assistant-compose-input-{}.json", idx + 1));
        let content = serde_json::to_vec_pretty(attempt)
            .map_err(|err| format!("failed to encode compose payload: {err}"))?;
        tokio::fs::write(&path, content)
            .await
            .map_err(|err| format!("failed to write compose payload artifact: {err}"))?;
        input_refs.push(serde_json::json!({
            "ref_id": ref_id,
            "path": path.to_string_lossy(),
        }));
    }

    let output_ref_id = format!("assistant-compose-output:{}", run_id.0);
    let output_path = run_dir.join("assistant-compose-output.txt");
    tokio::fs::write(&output_path, final_message.as_bytes())
        .await
        .map_err(|err| format!("failed to write compose output artifact: {err}"))?;

    let record = serde_json::json!({
        "input_refs": input_refs,
        "output_ref": {
            "ref_id": output_ref_id,
            "path": output_path.to_string_lossy(),
        },
        "output_ref_ids": output_ref_ids,
        "source_urls": source_urls,
        "quality_gate": quality_gate,
        "grounding_gate": grounding_gate,
        "planner_followup_signal_code": planner_followup_signal.map(PlannerGuidanceSignal::code),
    });
    event_log
        .append_app_event("compose", "compose_audit", "info", run_id, now_ms(), record)
        .await
        .map_err(|err| format!("failed to append compose audit event: {err}"))
}

async fn collect_source_urls_from_refs(
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut seen = BTreeSet::new();
    for outcome in &response_input.tool_outcomes {
        for metadata in &outcome.refs {
            if metadata.byte_count == 0 {
                continue;
            }
            let Some((content, _, _)) =
                resolve_ref_summary_input(&metadata.ref_id, render_refs).await
            else {
                continue;
            };
            for url in extract_plain_urls_from_text(&content) {
                if seen.insert(url.clone()) {
                    urls.push(url);
                }
                if urls.len() >= 8 {
                    return urls;
                }
            }
        }
    }
    urls
}

fn collect_source_urls_from_extracted_evidence(
    evidence_records: &[sieve_llm::ResponseEvidenceRecord],
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut seen = BTreeSet::new();
    for record in evidence_records {
        for url in &record.source_urls {
            if seen.insert(url.clone()) {
                urls.push(url.clone());
            }
            if urls.len() >= 8 {
                return urls;
            }
        }
        if let Some(candidate) = &record.answer_candidate {
            if let Some(url) = &candidate.url {
                if seen.insert(url.clone()) {
                    urls.push(url.clone());
                }
                if urls.len() >= 8 {
                    return urls;
                }
            }
        }
        for item in &record.items {
            if let Some(url) = &item.url {
                if seen.insert(url.clone()) {
                    urls.push(url.clone());
                }
                if urls.len() >= 8 {
                    return urls;
                }
            }
        }
    }
    urls
}

async fn build_compose_evidence_summaries(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut seen = BTreeSet::new();
    for (idx, metadata) in response_input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|metadata| metadata.byte_count > 0)
        .enumerate()
    {
        if idx >= 4 {
            break;
        }
        if !seen.insert(metadata.ref_id.clone()) {
            continue;
        }
        let Some((content, _, _)) = resolve_ref_summary_input(&metadata.ref_id, render_refs).await
        else {
            continue;
        };
        let cache_key = format!(
            "{}:{}:{}:{}",
            trusted_user_message, metadata.ref_id, metadata.byte_count, metadata.line_count
        );
        if let Some(summary) = evidence_cache.get(&cache_key) {
            if !summary.trim().is_empty() {
                summaries.push(summary.clone());
            }
            continue;
        }
        let payload = serde_json::json!({
            "task": "compose_evidence_extract",
            "trusted_user_message": trusted_user_message,
            "ref_id": metadata.ref_id,
            "content": content,
        });
        let ref_id = format!("assistant-compose-evidence:{}:{}", run_id.0, idx + 1);
        if let Some(summary) = summarize_with_ref_id_counted(
            summary_model,
            run_id,
            &ref_id,
            &payload,
            summary_calls,
            summary_budget,
        )
        .await
        {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                summaries.push(trimmed.to_string());
                evidence_cache.insert(cache_key, trimmed.to_string());
            }
        }
    }
    summaries
}

async fn run_compose_gate(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    trusted_evidence: &[String],
    composed_message: &str,
    extracted_evidence: &[sieve_llm::ResponseEvidenceRecord],
    evidence_summaries: &[String],
    source_urls: &[String],
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Option<ComposeGateOutput> {
    let payload = serde_json::json!({
        "task": "compose_gate",
        "trusted_user_message": trusted_user_message,
        "user_requested_sources": user_requested_sources(trusted_user_message),
        "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
        "trusted_evidence": trusted_evidence,
        "composed_message": composed_message,
        "extracted_evidence": extracted_evidence,
        "evidence_summaries": evidence_summaries,
        "source_urls": source_urls,
    });
    let raw = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-compose-gate:{}", run_id.0),
        &payload,
        summary_calls,
        summary_budget,
    )
    .await;
    parse_compose_gate_output(raw.as_deref())
}

pub(crate) async fn compose_assistant_message(
    summary_model: &dyn SummaryModel,
    sieve_home: &Path,
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    draft_message: String,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_budget: usize,
) -> ComposeAssistantOutcome {
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
    source_urls = filter_non_asset_urls(dedupe_preserve_order(source_urls));
    let trusted_evidence = extract_trusted_evidence_lines(
        trusted_user_message,
        response_input.planner_thoughts.as_deref(),
    );
    let extracted_evidence = response_input.extracted_evidence.clone();
    let evidence_summaries = if extracted_evidence.is_empty() {
        build_compose_evidence_summaries(
            summary_model,
            run_id,
            trusted_user_message,
            response_input,
            render_refs,
            evidence_cache,
            &mut summary_calls,
            summary_budget,
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
    let tool_outcomes: Vec<serde_json::Value> = response_input
        .tool_outcomes
        .iter()
        .map(|outcome| {
            serde_json::json!({
                "tool_name": outcome.tool_name,
                "outcome": outcome.outcome,
                "attempted_command": outcome.attempted_command,
                "failure_reason": outcome.failure_reason,
                "refs": outcome.refs.iter().map(|ref_metadata| {
                    serde_json::json!({
                        "ref_id": ref_metadata.ref_id,
                        "kind": ref_metadata.kind,
                        "byte_count": ref_metadata.byte_count,
                        "line_count": ref_metadata.line_count,
                    })
                }).collect::<Vec<_>>()
            })
        })
        .collect();

    let mut attempt_payloads = Vec::new();
    let payload = serde_json::json!({
        "task": "compose_user_reply",
        "trusted_user_message": trusted_user_message,
        "response_modality": response_input.response_modality,
        "user_requested_sources": user_requested_sources(trusted_user_message),
        "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
        "trusted_evidence": trusted_evidence.clone(),
        "assistant_draft_message": draft_message,
        "planner_thoughts": response_input.planner_thoughts.clone(),
        "tool_outcomes": tool_outcomes,
        "extracted_evidence": extracted_evidence.clone(),
        "output_ref_ids": output_ref_ids.clone(),
        "available_plain_urls": source_urls.clone(),
        "evidence_summaries": evidence_summaries.clone(),
    });
    attempt_payloads.push(payload.clone());

    let first_composed = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-compose:{}", run_id.0),
        &payload,
        &mut summary_calls,
        summary_budget,
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
    let mut gate = run_compose_gate(
        summary_model,
        run_id,
        trusted_user_message,
        &trusted_evidence,
        &composed,
        &extracted_evidence,
        &evidence_summaries,
        &source_urls,
        &mut summary_calls,
        summary_budget,
    )
    .await;
    let mut retry_diagnostics = Vec::new();
    if let Some(diagnostic) =
        compose_gate_requires_retry(&composed, trusted_user_message, gate.as_ref())
    {
        retry_diagnostics.push(diagnostic);
    }
    let did_retry = !retry_diagnostics.is_empty() && summary_calls < summary_budget;
    if did_retry {
        let retry_diagnostic = retry_diagnostics.join(" | ");
        let retry_payload = serde_json::json!({
            "task": "compose_user_reply",
            "trusted_user_message": trusted_user_message,
            "response_modality": response_input.response_modality,
            "user_requested_sources": user_requested_sources(trusted_user_message),
            "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
            "trusted_evidence": trusted_evidence.clone(),
            "assistant_draft_message": payload
                .get("assistant_draft_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            "planner_thoughts": response_input.planner_thoughts.clone(),
            "tool_outcomes": payload
                .get("tool_outcomes")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            "extracted_evidence": extracted_evidence.clone(),
            "output_ref_ids": output_ref_ids.clone(),
            "available_plain_urls": source_urls.clone(),
            "evidence_summaries": evidence_summaries.clone(),
            "compose_diagnostic": retry_diagnostic,
            "previous_composed_message": composed,
        });
        attempt_payloads.push(retry_payload.clone());
        composed = summarize_with_ref_id_counted(
            summary_model,
            run_id,
            &format!("assistant-compose-retry:{}", run_id.0),
            &retry_payload,
            &mut summary_calls,
            summary_budget,
        )
        .await
        .unwrap_or_else(|| {
            retry_payload
                .get("previous_composed_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        gate = run_compose_gate(
            summary_model,
            run_id,
            trusted_user_message,
            &trusted_evidence,
            &composed,
            &extracted_evidence,
            &evidence_summaries,
            &source_urls,
            &mut summary_calls,
            summary_budget,
        )
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
    let combined_gate = combine_gate_reasons(&[quality_gate.clone()]);
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
    if let Err(err) = write_compose_audit_artifacts(
        sieve_home,
        event_log,
        run_id,
        &attempt_payloads,
        &composed,
        &output_ref_ids,
        &source_urls,
        quality_gate.as_deref(),
        grounding_gate.as_deref(),
        planner_followup_signal,
    )
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
