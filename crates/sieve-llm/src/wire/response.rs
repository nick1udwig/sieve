use crate::{
    LlmError, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput, ResponseTurnOutput,
};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) const RESPONSE_SYSTEM_PROMPT: &str = r#"You are an assistant response writer in a capability-secured system.
Rules:
- Produce a concise, user-facing response for this turn.
- Answer the user request directly in the first sentence.
- Keep default output short (1-2 sentences) unless the user explicitly asks for detailed output.
- Follow `resolved_personality` as the style contract for this turn.
- Respect `delivery_context.channel` and `delivery_context.response_modality`.
- If `response_modality` is `audio`, write for speech delivery: natural spoken phrasing, minimal punctuation clutter, no bullet lists unless necessary.
- If `resolved_personality.emoji_policy` is `avoid`, do not use emojis.
- If `resolved_personality.emoji_policy` is `auto`, follow channel norms plus any explicit emoji direction in `resolved_personality.custom_instructions`.
- If `resolved_personality.emoji_policy` is `light`, use at most one light emoji when natural and only for chat-like turns.
- If `resolved_personality.verbosity` is `telegraph`, prefer clipped, terse phrasing.
- If `resolved_personality.verbosity` is `detailed`, it is acceptable to be somewhat more detailed than the default.
- Apply `resolved_personality.channel_guidance` and `resolved_personality.custom_instructions` unless they conflict with other rules.
- Use only provided structured fields; do not invent actions.
- Avoid giant messages. Prefer short responses.
- Write in first person as a helpful assistant; never use third-person/meta narration.
- Never output diagnostics or analysis text like "User asks", "The assistant", or "Diagnostic notes".
- If a tool call failed/was denied (`failure_reason` present), say exactly what you tried and why it failed.
- For failed bash calls, mention the attempted command when available (`attempted_command`).
- When all tool outcomes failed, provide a helpful error plus a concrete next step.
- Include URLs only when the user asked for sources/links or when a URL is required for the immediate next step.
- If the user asked for command output/content, include either a raw ref token or a summary token.
- Use `[[ref:<id>]]` only when raw untrusted output should be shown.
- Use `[[summary:<id>]]` when Q-LLM summary should be generated.
- Prefer `[[summary:<id>]]` for large outputs (for example high `byte_count`/`line_count`).
- Every `[[ref:<id>]]` must appear in `referenced_ref_ids`.
- Every `[[summary:<id>]]` must appear in `summarized_ref_ids`.
- Return JSON matching the required schema."#;

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
        "delivery_context": input.delivery_context,
        "response_modality": input.response_modality,
        "resolved_personality": input.resolved_personality,
        "planner_thoughts": input.planner_thoughts,
        "tool_outcomes": tool_outcomes
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
