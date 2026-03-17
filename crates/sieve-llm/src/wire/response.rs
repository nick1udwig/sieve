use crate::{
    LlmError, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput, ResponseTurnOutput,
};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize)]
struct ResponseTurnInputPayload<'a> {
    run_id: &'a str,
    trusted_user_message: &'a str,
    response_modality: &'a sieve_types::InteractionModality,
    planner_thoughts: Option<&'a str>,
    tool_outcomes: Vec<ResponseToolOutcomePayload<'a>>,
    trusted_effects: &'a [sieve_types::TrustedToolEffect],
    extracted_evidence: &'a [crate::ResponseEvidenceRecord],
}

#[derive(Serialize)]
struct ResponseToolOutcomePayload<'a> {
    tool_name: &'a str,
    outcome: &'a str,
    attempted_command: Option<&'a str>,
    failure_reason: Option<&'a str>,
    refs: Vec<ResponseRefMetadataPayload<'a>>,
}

#[derive(Serialize)]
struct ResponseRefMetadataPayload<'a> {
    ref_id: &'a str,
    kind: &'a str,
    byte_count: u64,
    line_count: u64,
}

pub(crate) fn serialize_response_input(input: &ResponseTurnInput) -> Result<Value, LlmError> {
    if input.trusted_user_message.trim().is_empty() {
        return Err(LlmError::Boundary(
            "empty trusted_user_message for response writer".to_string(),
        ));
    }

    let tool_outcomes = input
        .tool_outcomes
        .iter()
        .map(serialize_response_tool_outcome)
        .collect();

    serde_json::to_value(ResponseTurnInputPayload {
        run_id: &input.run_id.0,
        trusted_user_message: &input.trusted_user_message,
        response_modality: &input.response_modality,
        planner_thoughts: input.planner_thoughts.as_deref(),
        tool_outcomes,
        trusted_effects: &input.trusted_effects,
        extracted_evidence: &input.extracted_evidence,
    })
    .map_err(|err| LlmError::Boundary(format!("failed to serialize response input: {err}")))
}

fn serialize_response_tool_outcome(outcome: &ResponseToolOutcome) -> ResponseToolOutcomePayload<'_> {
    let refs = outcome
        .refs
        .iter()
        .map(serialize_response_ref_metadata)
        .collect();
    ResponseToolOutcomePayload {
        tool_name: &outcome.tool_name,
        outcome: &outcome.outcome,
        attempted_command: outcome.attempted_command.as_deref(),
        failure_reason: outcome.failure_reason.as_deref(),
        refs,
    }
}

fn serialize_response_ref_metadata(metadata: &ResponseRefMetadata) -> ResponseRefMetadataPayload<'_> {
    ResponseRefMetadataPayload {
        ref_id: &metadata.ref_id,
        kind: &metadata.kind,
        byte_count: metadata.byte_count,
        line_count: metadata.line_count,
    }
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
