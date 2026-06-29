use super::ComposeContext;
use crate::json_util::to_json_value;
use crate::render_refs::{resolve_ref_summary_input, RenderRef};
use crate::turn::summarize_with_ref_id_counted;
use serde::Serialize;
use sieve_llm::ResponseTurnInput;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize)]
struct ComposeEvidenceExtractPayload<'a> {
    task: &'static str,
    trusted_user_message: &'a str,
    ref_id: &'a str,
    content: String,
}

pub(super) async fn build_compose_evidence_summaries(
    context: &ComposeContext<'_>,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_calls: &mut usize,
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
        let payload = to_json_value(
            ComposeEvidenceExtractPayload {
                task: "compose_evidence_extract",
                trusted_user_message,
                ref_id: &metadata.ref_id,
                content,
            },
            "compose evidence extract payload",
        );
        let ref_id = format!(
            "assistant-compose-evidence:{}:{}",
            context.run_id.0,
            idx + 1
        );
        if let Some(summary) = summarize_with_ref_id_counted(
            context.summary_model,
            context.run_id,
            &ref_id,
            &payload,
            summary_calls,
            context.summary_budget,
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
