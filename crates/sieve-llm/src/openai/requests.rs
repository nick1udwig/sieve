use crate::wire::{
    guidance_output_schema, response_output_schema, GUIDANCE_SYSTEM_PROMPT, RESPONSE_SYSTEM_PROMPT,
    SUMMARY_SYSTEM_PROMPT,
};
use crate::{ResponseTurnInput, SummaryRequest};
use serde::Serialize;
use serde_json::{json, Value};
use sieve_types::PlannerGuidanceInput;

#[derive(Serialize)]
struct OpenAiMessage {
    role: &'static str,
    content: String,
}

#[derive(Serialize)]
struct OpenAiJsonSchemaSpec {
    name: &'static str,
    strict: bool,
    schema: Value,
}

#[derive(Serialize)]
struct OpenAiResponseFormat {
    #[serde(rename = "type")]
    format_type: &'static str,
    json_schema: OpenAiJsonSchemaSpec,
}

#[derive(Serialize)]
struct OpenAiStructuredRequest {
    model: String,
    temperature: u8,
    messages: Vec<OpenAiMessage>,
    response_format: OpenAiResponseFormat,
}

#[derive(Serialize)]
struct PlannerGuidanceRequestPayload {
    run_id: String,
    prompt: String,
}

#[derive(Serialize)]
struct SummaryRequestPayload {
    run_id: String,
    ref_id: String,
    byte_count: u64,
    line_count: u64,
    content: String,
}

#[derive(Serialize)]
struct SummaryResponseSchema {
    #[serde(rename = "type")]
    schema_type: &'static str,
    #[serde(rename = "additionalProperties")]
    additional_properties: bool,
    properties: Value,
    required: [&'static str; 1],
}

fn structured_request(
    model: &str,
    system_prompt: &'static str,
    user_content: String,
    schema_name: &'static str,
    schema: Value,
) -> Value {
    serde_json::to_value(OpenAiStructuredRequest {
        model: model.to_string(),
        temperature: 0,
        messages: vec![
            OpenAiMessage {
                role: "system",
                content: system_prompt.to_string(),
            },
            OpenAiMessage {
                role: "user",
                content: user_content,
            },
        ],
        response_format: OpenAiResponseFormat {
            format_type: "json_schema",
            json_schema: OpenAiJsonSchemaSpec {
                name: schema_name,
                strict: true,
                schema,
            },
        },
    })
    .expect("openai request serialization should succeed")
}

pub(super) fn build_guidance_request(input: PlannerGuidanceInput, model: &str) -> Value {
    let payload = serde_json::to_string(&PlannerGuidanceRequestPayload {
        run_id: input.run_id.0,
        prompt: input.prompt,
    })
    .expect("planner guidance payload serialization should succeed");
    structured_request(
        model,
        GUIDANCE_SYSTEM_PROMPT,
        payload,
        "planner_guidance_output",
        guidance_output_schema(),
    )
}

pub(super) fn build_response_request(
    input: &ResponseTurnInput,
    model: &str,
) -> Result<Value, crate::LlmError> {
    let response_payload = crate::wire::serialize_response_input(input)?;
    Ok(structured_request(
        model,
        RESPONSE_SYSTEM_PROMPT,
        response_payload.to_string(),
        "assistant_turn_response",
        response_output_schema(),
    ))
}

pub(super) fn build_summary_request(request: SummaryRequest, model: &str) -> Value {
    let response_schema = serde_json::to_value(SummaryResponseSchema {
        schema_type: "object",
        additional_properties: false,
        properties: json!({
            "summary": {"type":"string"}
        }),
        required: ["summary"],
    })
    .expect("summary response schema serialization should succeed");
    let payload = serde_json::to_string(&SummaryRequestPayload {
        run_id: request.run_id.0,
        ref_id: request.ref_id,
        byte_count: request.byte_count,
        line_count: request.line_count,
        content: request.content,
    })
    .expect("summary payload serialization should succeed");
    structured_request(
        model,
        SUMMARY_SYSTEM_PROMPT,
        payload,
        "untrusted_ref_summary",
        response_schema,
    )
}
