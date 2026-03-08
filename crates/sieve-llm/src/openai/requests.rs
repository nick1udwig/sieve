use crate::wire::{
    guidance_output_schema, response_output_schema, GUIDANCE_SYSTEM_PROMPT, RESPONSE_SYSTEM_PROMPT,
    SUMMARY_SYSTEM_PROMPT,
};
use crate::{ResponseTurnInput, SummaryRequest};
use serde_json::{json, Value};
use sieve_types::PlannerGuidanceInput;
pub(super) fn build_guidance_request(input: PlannerGuidanceInput, model: &str) -> Value {
    json!({
        "model": model,
        "temperature": 0,
        "messages": [
            {"role":"system","content": GUIDANCE_SYSTEM_PROMPT},
            {"role":"user","content": json!({
                "run_id": input.run_id.0,
                "prompt": input.prompt
            }).to_string()}
        ],
        "response_format": {
            "type":"json_schema",
            "json_schema": {
                "name":"planner_guidance_output",
                "strict": true,
                "schema": guidance_output_schema()
            }
        }
    })
}

pub(super) fn build_response_request(
    input: &ResponseTurnInput,
    model: &str,
) -> Result<Value, crate::LlmError> {
    let response_payload = crate::wire::serialize_response_input(input)?;
    Ok(json!({
        "model": model,
        "temperature": 0,
        "messages": [
            {"role":"system","content": RESPONSE_SYSTEM_PROMPT},
            {"role":"user","content": response_payload.to_string()}
        ],
        "response_format": {
            "type":"json_schema",
            "json_schema": {
                "name":"assistant_turn_response",
                "strict": true,
                "schema": response_output_schema()
            }
        }
    }))
}

pub(super) fn build_summary_request(request: SummaryRequest, model: &str) -> Value {
    let response_schema = json!({
        "type":"object",
        "additionalProperties": false,
        "properties": {
            "summary": {"type":"string"}
        },
        "required": ["summary"]
    });
    let payload = json!({
        "run_id": request.run_id.0,
        "ref_id": request.ref_id,
        "byte_count": request.byte_count,
        "line_count": request.line_count,
        "content": request.content,
    });
    json!({
        "model": model,
        "temperature": 0,
        "messages": [
            {"role":"system","content": SUMMARY_SYSTEM_PROMPT},
            {"role":"user","content": payload.to_string()}
        ],
        "response_format": {
            "type":"json_schema",
            "json_schema": {
                "name":"untrusted_ref_summary",
                "strict": true,
                "schema": response_schema
            }
        }
    })
}
