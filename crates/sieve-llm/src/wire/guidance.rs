use crate::LlmError;
use serde_json::{json, Value};
use sieve_types::{PlannerGuidanceOutput, PlannerGuidanceSignal};

pub(crate) const GUIDANCE_SYSTEM_PROMPT: &str = r#"Classify planner next-step guidance using numeric typed signals only.
Rules:
- Return JSON only matching schema.
- Prefer continue codes (100-113) when additional tool actions may still recover missing facts.
- Use final/stop codes only when further tool actions are unlikely to improve the answer.
- For factual/time-bound requests, if current evidence looks like discovery/search snippets or URL listings without fetched primary content, prefer continue (`110` or `108`) rather than final.
- Use `110` when a primary content fetch is still missing, `102` when one source exists but corroboration is needed, and `108` when quality is low.
- `guidance.code` must be one of:
  - 100 continue_need_evidence
  - 101 continue_fetch_primary_source
  - 102 continue_fetch_additional_source
  - 103 continue_refine_approach
  - 104 continue_need_required_parameter
  - 105 continue_need_fresh_or_time_bound_evidence
  - 106 continue_need_preference_or_constraint
  - 107 continue_tool_denied_try_alternative_allowed_tool
  - 108 continue_need_higher_quality_source
  - 109 continue_resolve_source_conflict
  - 110 continue_need_primary_content_fetch
  - 111 continue_need_url_extraction
  - 112 continue_need_canonical_non_asset_url
  - 113 continue_no_progress_try_different_action
  - 200 final_answer_ready
  - 201 final_answer_partial
  - 202 final_insufficient_evidence
  - 203 final_single_fact_ready
  - 204 final_conflicting_facts_with_range
  - 205 final_no_tool_action_needed
  - 300 stop_policy_blocked
  - 301 stop_budget_exhausted
  - 302 stop_no_allowed_tool_can_satisfy_task
  - 900 error_contract_violation
- `confidence_bps` must be 0..10000.
- Never output free-form strings outside numeric fields."#;

pub(crate) fn guidance_output_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "guidance": {
                "type":"object",
                "additionalProperties": false,
                "properties":{
                    "code":{"type":"integer","minimum":0,"maximum":65535},
                    "confidence_bps":{"type":"integer","minimum":0,"maximum":10000},
                    "source_hit_index":{"type":["integer","null"],"minimum":0,"maximum":65535},
                    "evidence_ref_index":{"type":["integer","null"],"minimum":0,"maximum":65535}
                },
                "required":["code","confidence_bps","source_hit_index","evidence_ref_index"]
            }
        },
        "required":["guidance"]
    })
}

pub(crate) fn decode_guidance_output(
    content_json: Value,
) -> Result<PlannerGuidanceOutput, LlmError> {
    let output: PlannerGuidanceOutput = serde_json::from_value(content_json)
        .map_err(|e| LlmError::Decode(format!("invalid guidance output payload: {e}")))?;

    PlannerGuidanceSignal::try_from(output.guidance.code)
        .map_err(|err| LlmError::Boundary(format!("invalid guidance signal: {err}")))?;

    if output.guidance.confidence_bps > 10_000 {
        return Err(LlmError::Boundary(format!(
            "guidance.confidence_bps out of range: {}",
            output.guidance.confidence_bps
        )));
    }

    Ok(output)
}
