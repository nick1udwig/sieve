use crate::{
    LlmError, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput, ResponseTurnOutput,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) const RESPONSE_SYSTEM_PROMPT: &str = include_str!("../prompts/response_system.md");

pub(crate) fn response_output_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties": {
            "message": {
                "type":"string"
            },
            "referenced_ref_ids": {
                "type":"array",
                "items": { "type":"string" }
            },
            "summarized_ref_ids": {
                "type":"array",
                "items": { "type":"string" }
            }
        },
        "required": ["message", "referenced_ref_ids", "summarized_ref_ids"]
    })
}

pub(crate) fn serialize_response_input(input: &ResponseTurnInput) -> Result<Value, LlmError> {
    if input.trusted_user_message.trim().is_empty() {
        return Err(LlmError::Boundary(
            "empty trusted_user_message for response writer".to_string(),
        ));
    }

    let tool_outcomes: Vec<Value> = input
        .tool_outcomes
        .iter()
        .map(serialize_response_tool_outcome)
        .collect();

    Ok(json!({
        "run_id": input.run_id.0,
        "trusted_user_message": input.trusted_user_message,
        "response_modality": input.response_modality,
        "planner_thoughts": input.planner_thoughts,
        "tool_outcomes": tool_outcomes,
        "trusted_effects": input.trusted_effects,
        "extracted_evidence": input.extracted_evidence
    }))
}

fn serialize_response_tool_outcome(outcome: &ResponseToolOutcome) -> Value {
    let refs: Vec<Value> = outcome
        .refs
        .iter()
        .map(serialize_response_ref_metadata)
        .collect();
    json!({
        "tool_name": outcome.tool_name,
        "outcome": outcome.outcome,
        "attempted_command": outcome.attempted_command,
        "failure_reason": outcome.failure_reason,
        "refs": refs,
    })
}

fn serialize_response_ref_metadata(metadata: &ResponseRefMetadata) -> Value {
    json!({
        "ref_id": metadata.ref_id,
        "kind": metadata.kind,
        "byte_count": metadata.byte_count,
        "line_count": metadata.line_count,
    })
}

#[derive(Debug, Deserialize)]
struct ResponseTurnOutputWire {
    message: String,
    #[serde(default)]
    referenced_ref_ids: Vec<String>,
    #[serde(default)]
    summarized_ref_ids: Vec<String>,
}

pub(crate) fn decode_response_output(content_json: Value) -> Result<ResponseTurnOutput, LlmError> {
    let decoded: ResponseTurnOutputWire = serde_json::from_value(content_json)
        .map_err(|e| LlmError::Decode(format!("invalid response output payload: {e}")))?;

    if decoded.message.trim().is_empty() {
        return Err(LlmError::Boundary(
            "response writer returned empty message".to_string(),
        ));
    }

    Ok(ResponseTurnOutput {
        message: decoded.message,
        referenced_ref_ids: decoded.referenced_ref_ids.into_iter().collect(),
        summarized_ref_ids: decoded.summarized_ref_ids.into_iter().collect(),
    })
}
