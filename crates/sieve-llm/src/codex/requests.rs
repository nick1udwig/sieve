use crate::wire::{
    guidance_output_schema, response_output_schema, GUIDANCE_SYSTEM_PROMPT, PLANNER_SYSTEM_PROMPT,
    RESPONSE_SYSTEM_PROMPT, SUMMARY_SYSTEM_PROMPT,
};
use crate::{LlmError, ResponseTurnInput, SummaryRequest};
use serde_json::{json, Value};
use sieve_tool_contracts::tool_args_schema;
use sieve_types::PlannerGuidanceInput;

const DEFAULT_CODEX_API_BASE: &str = "https://chatgpt.com/backend-api";

pub(crate) fn build_planner_request(
    model: &str,
    messages: Vec<Value>,
    allowed_tools: &[String],
) -> Result<Value, LlmError> {
    let (instructions, input) = split_messages(messages)?;
    let mut body = json!({
        "model": model,
        "store": false,
        "instructions": instructions.unwrap_or_else(|| PLANNER_SYSTEM_PROMPT.to_string()),
        "input": input,
    });

    let tools = planner_tool_definitions(allowed_tools)?;
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = Value::String("auto".to_string());
        body["parallel_tool_calls"] = Value::Bool(true);
    }

    Ok(body)
}

pub(crate) fn build_guidance_request(input: PlannerGuidanceInput, model: &str) -> Value {
    build_structured_request(
        model,
        GUIDANCE_SYSTEM_PROMPT,
        json!({
            "run_id": input.run_id.0,
            "prompt": input.prompt
        }),
        "planner_guidance_output",
        guidance_output_schema(),
    )
}

pub(crate) fn build_response_request(
    input: &ResponseTurnInput,
    model: &str,
) -> Result<Value, LlmError> {
    let response_payload = crate::wire::serialize_response_input(input)?;
    Ok(build_structured_request(
        model,
        RESPONSE_SYSTEM_PROMPT,
        response_payload,
        "assistant_turn_response",
        response_output_schema(),
    ))
}

pub(crate) fn build_summary_request(request: SummaryRequest, model: &str) -> Value {
    let payload = json!({
        "run_id": request.run_id.0,
        "ref_id": request.ref_id,
        "byte_count": request.byte_count,
        "line_count": request.line_count,
        "content": request.content,
    });
    build_structured_request(
        model,
        SUMMARY_SYSTEM_PROMPT,
        payload,
        "untrusted_ref_summary",
        json!({
            "type":"object",
            "additionalProperties": false,
            "properties": {
                "summary": {"type":"string"}
            },
            "required": ["summary"]
        }),
    )
}

pub(crate) fn resolve_codex_url(base_url: Option<&str>) -> String {
    let raw = base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CODEX_API_BASE);
    let normalized = raw.trim_end_matches('/');
    if normalized.ends_with("/codex/responses") {
        normalized.to_string()
    } else if normalized.ends_with("/codex") {
        format!("{normalized}/responses")
    } else {
        format!("{normalized}/codex/responses")
    }
}

fn build_structured_request(
    model: &str,
    instructions: &str,
    payload: Value,
    schema_name: &str,
    schema: Value,
) -> Value {
    json!({
        "model": model,
        "store": false,
        "instructions": instructions,
        "input": [
            {
                "role":"user",
                "content": [
                    {
                        "type":"input_text",
                        "text": payload.to_string()
                    }
                ]
            }
        ],
        "text": {
            "format": {
                "type":"json_schema",
                "name": schema_name,
                "strict": true,
                "schema": schema
            }
        }
    })
}

fn split_messages(messages: Vec<Value>) -> Result<(Option<String>, Vec<Value>), LlmError> {
    let mut instructions = None;
    let mut input = Vec::new();

    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::Decode("planner message missing role".to_string()))?;
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                LlmError::Decode("planner message missing string content".to_string())
            })?;

        if role == "system" && instructions.is_none() {
            instructions = Some(content.to_string());
            continue;
        }

        input.push(json!({
            "role": role,
            "content": [
                {
                    "type":"input_text",
                    "text": content
                }
            ]
        }));
    }

    Ok((instructions, input))
}

fn planner_tool_definitions(allowed_tools: &[String]) -> Result<Vec<Value>, LlmError> {
    let mut tools = Vec::with_capacity(allowed_tools.len());
    for tool_name in allowed_tools {
        let schema = tool_args_schema(tool_name).ok_or_else(|| {
            LlmError::Boundary(format!(
                "allowed tool `{tool_name}` is missing a contract schema"
            ))
        })?;
        tools.push(json!({
            "type": "function",
            "name": tool_name,
            "parameters": schema,
            "strict": true
        }));
    }
    Ok(tools)
}
