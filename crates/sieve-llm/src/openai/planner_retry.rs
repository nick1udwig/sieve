use crate::wire::{
    decode_planner_output, extract_openai_planner_output_json,
    planner_regeneration_diagnostic_prompt, PlannerDecodeOutcome,
};
use crate::LlmError;
use reqwest::StatusCode;
use serde_json::{json, Value};
use sieve_tool_contracts::tool_args_schema;
use sieve_types::{PlannerTurnOutput, ToolContractValidationReport};
use std::future::Future;
use std::time::Duration;

pub(super) async fn run_planner_with_one_regeneration<F, Fut>(
    model: &str,
    messages: Vec<Value>,
    allowed_tools: &[String],
    send_request: F,
) -> Result<PlannerTurnOutput, LlmError>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, LlmError>>,
{
    run_planner_with_one_regeneration_with_builder(
        model,
        messages,
        allowed_tools,
        planner_chat_completion_request,
        send_request,
    )
    .await
}

pub(crate) async fn run_planner_with_one_regeneration_with_builder<F, Fut, B>(
    model: &str,
    mut messages: Vec<Value>,
    allowed_tools: &[String],
    build_request: B,
    mut send_request: F,
) -> Result<PlannerTurnOutput, LlmError>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, LlmError>>,
    B: Fn(&str, Vec<Value>, &[String]) -> Result<Value, LlmError>,
{
    let mut regenerated = false;

    loop {
        let request = build_request(model, messages.clone(), allowed_tools)?;
        let response = send_request(request).await?;
        let output_json = extract_openai_planner_output_json(&response)?;

        match decode_planner_output(output_json)? {
            PlannerDecodeOutcome::Valid(output) => {
                ensure_allowed_tools(allowed_tools, &output)?;
                return Ok(output);
            }
            PlannerDecodeOutcome::InvalidToolContracts(report) => {
                if regenerated {
                    return Err(regeneration_exhausted_error(report));
                }
                regenerated = true;
                let prompt = planner_regeneration_diagnostic_prompt(&report)?;
                messages.push(json!({"role":"user","content": prompt}));
            }
        }
    }
}

fn planner_chat_completion_request(
    model: &str,
    messages: Vec<Value>,
    allowed_tools: &[String],
) -> Result<Value, LlmError> {
    let tools = planner_tool_definitions(allowed_tools)?;
    if tools.is_empty() {
        return Ok(json!({
            "model": model,
            "temperature": 0,
            "messages": messages
        }));
    }

    Ok(json!({
        "model": model,
        "temperature": 0,
        "messages": messages,
        "tools": tools,
        "tool_choice": "auto"
    }))
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
            "function": {
                "name": tool_name,
                "description": tool_description(tool_name),
                "parameters": schema
            }
        }));
    }

    Ok(tools)
}

fn tool_description(tool_name: &str) -> &'static str {
    match tool_name {
        "automation" => {
            "Manage heartbeat/cron automation. Use for reminders, scheduling, listing, pausing, resuming, or removing cron jobs. For cron_add, pass schedule as an object: {kind:\"after\",delay:\"1m\"}, {kind:\"at\",timestamp:\"2026-03-08T12:00:00Z\"}, {kind:\"every\",interval:\"15m\"}, or {kind:\"cron\",expr:\"0 9 * * 1-5\"}."
        }
        "bash" => "Run a cataloged shell command through runtime policy gates.",
        "endorse" => "Raise integrity of a labeled value_ref after explicit approval.",
        "declassify" => "Create a derived release value_ref authorized for one exact sink and channel after explicit approval.",
        _ => "Planner tool",
    }
}

fn ensure_allowed_tools(
    allowed_tools: &[String],
    output: &PlannerTurnOutput,
) -> Result<(), LlmError> {
    for call in &output.tool_calls {
        if !allowed_tools
            .iter()
            .any(|allowed| allowed == &call.tool_name)
        {
            return Err(LlmError::Boundary(format!(
                "planner emitted disallowed tool `{}`",
                call.tool_name
            )));
        }
    }
    Ok(())
}

fn regeneration_exhausted_error(report: ToolContractValidationReport) -> LlmError {
    let serialized = serde_json::to_string(&report)
        .unwrap_or_else(|_| "{\"serialization\":\"failed\"}".to_string());
    LlmError::Boundary(format!(
        "planner emitted invalid tool args after one regeneration pass: {serialized}"
    ))
}

pub(crate) fn is_transient_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) || status.is_server_error()
}

pub(crate) fn backoff(base: Duration, attempt: usize) -> Duration {
    let shift = (attempt.saturating_sub(1)) as u32;
    let multiplier = 1u32.checked_shl(shift).unwrap_or(u32::MAX);
    base.saturating_mul(multiplier)
}

pub(crate) fn truncate_for_error(input: &str) -> String {
    const MAX: usize = 512;
    if input.len() <= MAX {
        input.to_string()
    } else {
        format!("{}...[truncated]", &input[..MAX])
    }
}
