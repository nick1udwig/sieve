use super::openai_envelope::ensure_not_refusal;
use crate::truncate;
use crate::LlmError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sieve_tool_contracts::validate_at_index;
use sieve_types::{
    PlannerToolCall, PlannerTurnOutput, SourceSpan, ToolContractValidationError,
    ToolContractValidationReport, TOOL_CONTRACTS_VERSION_V1,
};

mod input;

pub(crate) use input::build_planner_messages;
#[cfg(test)]
pub(crate) use input::serialize_planner_input;

pub(crate) const PLANNER_SYSTEM_PROMPT: &str = sieve_prompts::planner::SYSTEM;

pub(crate) enum PlannerDecodeOutcome {
    Valid(PlannerTurnOutput),
    InvalidToolContracts(ToolContractValidationReport),
}

#[derive(Serialize)]
struct NormalizedPlannerToolCall {
    tool_name: String,
    args: Value,
}

#[derive(Serialize)]
struct NormalizedPlannerOutput {
    thoughts: Option<String>,
    tool_calls: Vec<NormalizedPlannerToolCall>,
}

pub(crate) fn extract_openai_planner_output_json(response: &Value) -> Result<Value, LlmError> {
    ensure_not_refusal(response)?;

    if response.get("output").and_then(Value::as_array).is_some() {
        return extract_responses_planner_output_json(response);
    }

    let empty_tool_calls = Vec::new();
    let tool_calls = match response.pointer("/choices/0/message/tool_calls") {
        Some(Value::Array(tool_calls)) => tool_calls,
        Some(Value::Null) | None => &empty_tool_calls,
        Some(_) => {
            return Err(LlmError::Decode(
                "choices[0].message.tool_calls must be an array when present".to_string(),
            ))
        }
    };

    let mut normalized_tool_calls = Vec::with_capacity(tool_calls.len());
    for (idx, call) in tool_calls.iter().enumerate() {
        let tool_name = call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                LlmError::Decode(format!(
                    "missing choices[0].message.tool_calls[{idx}].function.name string"
                ))
            })?;

        let arguments_raw = call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                LlmError::Decode(format!(
                    "missing choices[0].message.tool_calls[{idx}].function.arguments string"
                ))
            })?;

        let arguments_json = serde_json::from_str::<Value>(arguments_raw).map_err(|err| {
            LlmError::Decode(format!(
                "invalid JSON in choices[0].message.tool_calls[{idx}].function.arguments: {err}"
            ))
        })?;
        let arguments = arguments_json.as_object().cloned().ok_or_else(|| {
            LlmError::Decode(format!(
                "tool call arguments at index {idx} must decode to an object"
            ))
        })?;

        normalized_tool_calls.push(NormalizedPlannerToolCall {
            tool_name: tool_name.to_string(),
            args: Value::Object(arguments),
        });
    }

    let thoughts = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    serde_json::to_value(NormalizedPlannerOutput {
        thoughts,
        tool_calls: normalized_tool_calls,
    })
    .map_err(|err| LlmError::Decode(format!("failed to serialize planner output: {err}")))
}

fn extract_responses_planner_output_json(response: &Value) -> Result<Value, LlmError> {
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::Decode("responses payload missing `output` array".to_string()))?;

    let mut normalized_tool_calls = Vec::new();
    let mut thoughts_parts = Vec::new();

    for (idx, item) in output.iter().enumerate() {
        match item.get("type").and_then(Value::as_str).unwrap_or_default() {
            "function_call" => {
                let tool_name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| {
                        LlmError::Decode(format!(
                            "missing output[{idx}].name string for function_call"
                        ))
                    })?;

                let arguments_raw =
                    item.get("arguments")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            LlmError::Decode(format!(
                                "missing output[{idx}].arguments string for function_call"
                            ))
                        })?;

                let arguments_json =
                    serde_json::from_str::<Value>(arguments_raw).map_err(|err| {
                        LlmError::Decode(format!("invalid JSON in output[{idx}].arguments: {err}"))
                    })?;
                let arguments = arguments_json.as_object().cloned().ok_or_else(|| {
                    LlmError::Decode(format!(
                        "function_call arguments at output[{idx}] must decode to an object"
                    ))
                })?;

                normalized_tool_calls.push(NormalizedPlannerToolCall {
                    tool_name: tool_name.to_string(),
                    args: Value::Object(arguments),
                });
            }
            "message" => {
                let Some(content) = item.get("content").and_then(Value::as_array) else {
                    continue;
                };
                for part in content {
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
                    if (part_type == "output_text" || part_type == "text")
                        && part.get("text").and_then(Value::as_str).is_some()
                    {
                        let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                        let text = text.trim();
                        if !text.is_empty() {
                            thoughts_parts.push(text.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let thoughts = if thoughts_parts.is_empty() {
        None
    } else {
        Some(thoughts_parts.join("\n"))
    };

    serde_json::to_value(NormalizedPlannerOutput {
        thoughts,
        tool_calls: normalized_tool_calls,
    })
    .map_err(|err| LlmError::Decode(format!("failed to serialize planner output: {err}")))
}

pub(crate) fn planner_regeneration_diagnostic_prompt(
    report: &ToolContractValidationReport,
) -> Result<String, LlmError> {
    let diagnostics = serde_json::to_string_pretty(report).map_err(|e| {
        LlmError::Decode(format!(
            "failed to serialize tool-contract diagnostics for regeneration: {e}"
        ))
    })?;

    Ok(sieve_prompts::planner::REGENERATION_DIAGNOSTIC.replace("{{DIAGNOSTICS}}", &diagnostics))
}

#[derive(Debug, Deserialize)]
struct PlannerTurnOutputWire {
    thoughts: Option<String>,
    tool_calls: Vec<PlannerToolCallWire>,
}

#[derive(Debug, Deserialize)]
struct PlannerToolCallWire {
    tool_name: String,
    #[serde(default)]
    args: Map<String, Value>,
}

pub(crate) fn decode_planner_output(content_json: Value) -> Result<PlannerDecodeOutcome, LlmError> {
    let decoded: PlannerTurnOutputWire =
        serde_json::from_value(content_json.clone()).map_err(|primary_err| {
            let preview = truncate_json_for_error(&content_json, 240);
            LlmError::Decode(format!(
                "invalid planner output payload: {primary_err}; payload={preview}"
            ))
        })?;

    let mut tool_calls = Vec::with_capacity(decoded.tool_calls.len());
    let mut errors = Vec::new();

    for (idx, tool) in decoded.tool_calls.into_iter().enumerate() {
        let args_value = Value::Object(tool.args.clone());
        if let Err(err) = validate_at_index(idx, &tool.tool_name, &args_value) {
            let mut diagnostic = err.as_validation_error();
            if diagnostic.hint.is_none() {
                diagnostic.hint = Some(default_contract_hint(&diagnostic));
            }
            diagnostic.span = recover_contract_span(&content_json, idx, &diagnostic.argument_path);
            errors.push(diagnostic);
        }

        tool_calls.push(PlannerToolCall {
            tool_name: tool.tool_name,
            args: tool.args.into_iter().collect(),
        });
    }

    if !errors.is_empty() {
        return Ok(PlannerDecodeOutcome::InvalidToolContracts(
            ToolContractValidationReport {
                contract_version: TOOL_CONTRACTS_VERSION_V1,
                errors,
            },
        ));
    }

    Ok(PlannerDecodeOutcome::Valid(PlannerTurnOutput {
        thoughts: decoded.thoughts,
        tool_calls,
    }))
}

fn truncate_json_for_error(value: &Value, max_chars: usize) -> String {
    let raw =
        serde_json::to_string(value).unwrap_or_else(|_| "<non-serializable-json>".to_string());
    truncate::truncate_for_error(&raw, max_chars)
}

fn recover_contract_span(
    root: &Value,
    tool_call_index: usize,
    argument_path: &str,
) -> Option<SourceSpan> {
    let tool_calls = root.pointer("/tool_calls")?.as_array()?;
    let target_call = tool_calls.get(tool_call_index)?;

    let source = serde_json::to_string(root).ok()?;
    let (call_start, call_source) =
        locate_tool_call_minified(&source, tool_calls, tool_call_index)?;

    let (value_start, value_end_exclusive) =
        locate_argument_value_range(&call_source, target_call, argument_path)?;

    Some(SourceSpan {
        line: 1,
        column: (call_start + value_start + 1) as u32,
        end_line: 1,
        end_column: (call_start + value_end_exclusive + 1) as u32,
    })
}

fn locate_tool_call_minified(
    source: &str,
    tool_calls: &[Value],
    target_index: usize,
) -> Option<(usize, String)> {
    let mut cursor = 0usize;
    for (idx, call) in tool_calls.iter().enumerate() {
        let call_source = serde_json::to_string(call).ok()?;
        let rel = source.get(cursor..)?.find(&call_source)?;
        let start = cursor + rel;
        if idx == target_index {
            return Some((start, call_source));
        }
        cursor = start + call_source.len();
    }
    None
}

fn locate_argument_value_range(
    call_source: &str,
    target_call: &Value,
    argument_path: &str,
) -> Option<(usize, usize)> {
    let args_value = target_call.pointer("/args")?;

    if argument_path == "/" {
        let args_source = serde_json::to_string(args_value).ok()?;
        let pattern = format!("\"args\":{args_source}");
        let offset = call_source.find(&pattern)?;
        let value_start = offset + "\"args\":".len();
        let value_end = value_start + args_source.len();
        return Some((value_start, value_end));
    }

    let key = argument_path.strip_prefix('/')?;
    if key.is_empty() {
        return None;
    }

    let args_object = args_value.as_object()?;
    let field_value = args_object.get(key)?;
    let key_source = serde_json::to_string(key).ok()?;
    let value_source = serde_json::to_string(field_value).ok()?;
    let pattern = format!("{key_source}:{value_source}");
    let offset = call_source.find(&pattern)?;
    let value_start = offset + key_source.len() + 1;
    let value_end = value_start + value_source.len();
    Some((value_start, value_end))
}

fn default_contract_hint(diagnostic: &ToolContractValidationError) -> String {
    if let Some(expected) = &diagnostic.expected {
        return format!(
            "set tool_calls[{}].args{} to {}",
            diagnostic.tool_call_index, diagnostic.argument_path, expected
        );
    }
    format!(
        "fix tool_calls[{}].args{} to satisfy contract",
        diagnostic.tool_call_index, diagnostic.argument_path
    )
}
