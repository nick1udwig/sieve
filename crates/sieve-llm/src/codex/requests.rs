use crate::json_util::to_json_value;
use crate::wire::{
    guidance_output_schema, response_output_schema, GUIDANCE_SYSTEM_PROMPT, PLANNER_SYSTEM_PROMPT,
    RESPONSE_SYSTEM_PROMPT, SUMMARY_SYSTEM_PROMPT,
};
use crate::{LlmError, ResponseTurnInput, SummaryRequest};
use serde::Serialize;
use serde_json::{json, Value};
use sieve_types::PlannerGuidanceInput;

mod schema;
mod types;

use schema::planner_tool_definitions;
use types::{
    CodexContentPart, CodexInputMessage, CodexResponsesRequest, CodexTextConfig, CodexTextFormat,
};

const DEFAULT_CODEX_API_BASE: &str = "https://chatgpt.com/backend-api";

#[derive(Serialize)]
struct PlannerGuidancePayload {
    run_id: String,
    prompt: String,
}

#[derive(Serialize)]
struct SummaryPayload {
    run_id: String,
    ref_id: String,
    byte_count: u64,
    line_count: u64,
    content: String,
}

pub(crate) fn build_planner_request(
    model: &str,
    messages: Vec<Value>,
    allowed_tools: &[String],
) -> Result<Value, LlmError> {
    let (instructions, input) = split_messages(messages)?;
    let tools = planner_tool_definitions(allowed_tools)?;
    let has_tools = !tools.is_empty();
    Ok(to_json_value(
        CodexResponsesRequest {
            model,
            store: false,
            stream: true,
            instructions: instructions.unwrap_or_else(|| PLANNER_SYSTEM_PROMPT.to_string()),
            input,
            tools: has_tools.then_some(tools),
            tool_choice: has_tools.then_some("auto"),
            parallel_tool_calls: has_tools.then_some(true),
            text: None,
        },
        "codex planner request",
    ))
}

pub(crate) fn build_guidance_request(input: PlannerGuidanceInput, model: &str) -> Value {
    build_structured_request(
        model,
        GUIDANCE_SYSTEM_PROMPT,
        to_json_value(
            PlannerGuidancePayload {
                run_id: input.run_id.0,
                prompt: input.prompt,
            },
            "codex guidance payload",
        ),
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
    build_structured_request(
        model,
        SUMMARY_SYSTEM_PROMPT,
        to_json_value(
            SummaryPayload {
                run_id: request.run_id.0,
                ref_id: request.ref_id,
                byte_count: request.byte_count,
                line_count: request.line_count,
                content: request.content,
            },
            "codex summary payload",
        ),
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
    to_json_value(
        CodexResponsesRequest {
            model,
            store: false,
            stream: true,
            instructions: instructions.to_string(),
            input: vec![CodexInputMessage {
                role: "user".to_string(),
                content: vec![CodexContentPart {
                    content_type: "input_text",
                    text: payload.to_string(),
                }],
            }],
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            text: Some(CodexTextConfig {
                format: CodexTextFormat {
                    format_type: "json_schema",
                    name: schema_name.to_string(),
                    strict: true,
                    schema,
                },
            }),
        },
        "codex structured request",
    )
}

fn split_messages(
    messages: Vec<Value>,
) -> Result<(Option<String>, Vec<CodexInputMessage>), LlmError> {
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

        let content_type = match role {
            "assistant" => "output_text",
            _ => "input_text",
        };
        input.push(CodexInputMessage {
            role: role.to_string(),
            content: vec![CodexContentPart {
                content_type,
                text: content.to_string(),
            }],
        });
    }

    Ok((instructions, input))
}

#[cfg(test)]
mod tests {
    use super::{
        build_guidance_request, build_planner_request, planner_tool_definitions, split_messages,
        GUIDANCE_SYSTEM_PROMPT,
    };
    use serde_json::json;
    use sieve_types::{PlannerGuidanceInput, RunId};

    #[test]
    fn planner_request_sets_stream_true() {
        let request = build_planner_request(
            "gpt-5.4",
            vec![
                json!({"role":"system","content":"planner"}),
                json!({"role":"user","content":"{\"ok\":true}"}),
            ],
            &[],
        )
        .expect("planner request");
        assert_eq!(request.get("stream").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn structured_requests_set_stream_true() {
        let request = build_guidance_request(
            PlannerGuidanceInput {
                run_id: RunId("run-1".to_string()),
                prompt: "ping".to_string(),
            },
            "gpt-5.4",
        );
        assert_eq!(request.get("stream").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn split_messages_keeps_first_system_as_instructions() {
        let (instructions, input) = split_messages(vec![
            json!({"role":"system","content": GUIDANCE_SYSTEM_PROMPT}),
            json!({"role":"user","content":"hello"}),
        ])
        .expect("split");
        assert_eq!(instructions.as_deref(), Some(GUIDANCE_SYSTEM_PROMPT));
        assert_eq!(input.len(), 1);
    }

    #[test]
    fn split_messages_encodes_assistant_history_as_output_text() {
        let (_, input) = split_messages(vec![
            json!({"role":"system","content":"planner"}),
            json!({"role":"user","content":"hello"}),
            json!({"role":"assistant","content":"previous reply"}),
        ])
        .expect("split");
        let input = serde_json::to_value(input).expect("input json");
        assert_eq!(
            input[0].pointer("/content/0/type"),
            Some(&json!("input_text"))
        );
        assert_eq!(
            input[1].pointer("/content/0/type"),
            Some(&json!("output_text"))
        );
    }

    #[test]
    fn planner_tool_definitions_flatten_codex_incompatible_one_of() {
        let tools = planner_tool_definitions(&["automation".to_string()]).expect("tools");
        let tools = serde_json::to_value(tools).expect("tools json");
        let schedule = tools[0]
            .pointer("/parameters/properties/schedule")
            .cloned()
            .expect("schedule schema");
        assert!(schedule.get("oneOf").is_none());
        assert_eq!(
            schedule.pointer("/properties/kind/enum"),
            Some(&json!(["after", "at", "every", "cron"]))
        );
        assert_eq!(
            schedule.pointer("/required"),
            Some(&json!(["delay", "expr", "interval", "kind", "timestamp"]))
        );
    }

    #[test]
    fn nullable_one_of_flattens_to_all_required_nullable_fields_for_codex() {
        let tools = planner_tool_definitions(&["automation".to_string()]).expect("tools");
        let tools = serde_json::to_value(tools).expect("tools json");
        let schedule = tools[0]
            .pointer("/parameters/properties/schedule")
            .cloned()
            .expect("schedule schema");
        assert_eq!(
            schedule.pointer("/required"),
            Some(&json!(["delay", "expr", "interval", "kind", "timestamp"]))
        );
        assert_eq!(
            schedule.pointer("/properties/delay/type"),
            Some(&json!(["string", "null"]))
        );
        assert_eq!(
            schedule.pointer("/properties/timestamp/type"),
            Some(&json!(["string", "null"]))
        );
    }

    #[test]
    fn codex_tool_schema_requires_all_top_level_properties() {
        let tools = planner_tool_definitions(&["automation".to_string()]).expect("tools");
        let tools = serde_json::to_value(tools).expect("tools json");
        let parameters = tools[0].pointer("/parameters").cloned().expect("params");
        assert_eq!(
            parameters.pointer("/required"),
            Some(&json!(["action", "job_id", "prompt", "schedule", "target"]))
        );
        assert_eq!(
            parameters.pointer("/properties/job_id/type"),
            Some(&json!(["string", "null"]))
        );
    }
}
