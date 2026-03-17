use super::openai_envelope::ensure_not_refusal;
use crate::LlmError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sieve_command_summaries::planner_command_catalog;
use sieve_tool_contracts::validate_at_index;
use sieve_types::{
    PlannerBrowserSession, PlannerCodexSession, PlannerConversationMessageKind,
    PlannerGuidanceSignal, PlannerToolCall, PlannerTurnInput, PlannerTurnOutput, RuntimeEvent,
    SourceSpan, ToolContractValidationError, ToolContractValidationReport,
    TOOL_CONTRACTS_VERSION_V1,
};

pub(crate) const PLANNER_SYSTEM_PROMPT: &str = sieve_prompts::planner::SYSTEM;

pub(crate) enum PlannerDecodeOutcome {
    Valid(PlannerTurnOutput),
    InvalidToolContracts(ToolContractValidationReport),
}

#[derive(Serialize)]
struct PlannerGuidancePayload {
    code: u16,
    signal_name: Option<&'static str>,
    confidence_bps: u16,
    source_hit_index: Option<u16>,
    evidence_ref_index: Option<u16>,
}

#[derive(Serialize, Default)]
struct PlannerGuidanceContract {
    #[serde(skip_serializing_if = "Option::is_none")]
    required_action_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forbidden_action_classes: Option<Vec<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_non_asset_target: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefer_markdown_view: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_action_change: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefer_current_browser_session: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avoid_recent_interstitial_origin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserve_task_target: Option<bool>,
}

#[derive(Serialize)]
struct PlannerCommandCatalogEntry<'a> {
    command: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct PlannerContextPayload<'a> {
    run_id: &'a str,
    trusted_user_message: &'a str,
    #[serde(rename = "CURRENT_TIME_UTC")]
    current_time_utc: Option<&'a str>,
    #[serde(rename = "CURRENT_TIMEZONE")]
    current_timezone: Option<&'a str>,
    #[serde(rename = "ALLOWED_NET_CONNECT_SCOPES")]
    allowed_net_connect_scopes: &'a [String],
    #[serde(rename = "BROWSER_SESSIONS")]
    browser_sessions: &'a [PlannerBrowserSession],
    #[serde(rename = "CODEX_SESSIONS")]
    codex_sessions: &'a [PlannerCodexSession],
    #[serde(rename = "BASH_COMMAND_CATALOG")]
    bash_command_catalog: Vec<PlannerCommandCatalogEntry<'a>>,
    previous_event_kinds: Vec<&'static str>,
    guidance: Option<PlannerGuidancePayload>,
    guidance_contract: Option<PlannerGuidanceContract>,
}

#[derive(Serialize)]
struct PlannerChatMessage {
    role: &'static str,
    content: String,
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

pub(crate) fn serialize_planner_input(input: &PlannerTurnInput) -> Result<Value, LlmError> {
    if input.user_message.trim().is_empty() {
        return Err(LlmError::Boundary("empty trusted user_message".to_string()));
    }

    let event_kinds: Vec<&'static str> = input
        .previous_events
        .iter()
        .map(runtime_event_kind)
        .collect();
    let guidance = input.guidance.as_ref().map(|guidance| {
        let signal_name = PlannerGuidanceSignal::try_from(guidance.code)
            .ok()
            .map(PlannerGuidanceSignal::name);
        PlannerGuidancePayload {
            code: guidance.code,
            signal_name,
            confidence_bps: guidance.confidence_bps,
            source_hit_index: guidance.source_hit_index,
            evidence_ref_index: guidance.evidence_ref_index,
        }
    });
    let guidance_contract = input
        .guidance
        .as_ref()
        .and_then(planner_guidance_contract_payload);
    let bash_command_catalog = planner_command_catalog_for_allowed_tools(&input.allowed_tools);
    serde_json::to_value(PlannerContextPayload {
        run_id: &input.run_id.0,
        trusted_user_message: &input.user_message,
        current_time_utc: input.current_time_utc.as_deref(),
        current_timezone: input.current_timezone.as_deref(),
        allowed_net_connect_scopes: &input.allowed_net_connect_scopes,
        browser_sessions: &input.browser_sessions,
        codex_sessions: &input.codex_sessions,
        bash_command_catalog,
        previous_event_kinds: event_kinds,
        guidance,
        guidance_contract,
    })
    .map_err(|err| LlmError::Boundary(format!("failed to serialize planner input: {err}")))
}

pub(crate) fn build_planner_messages(input: &PlannerTurnInput) -> Result<Vec<Value>, LlmError> {
    let context_payload = serialize_planner_input(input)?;
    let mut messages = vec![
        planner_message("system", PLANNER_SYSTEM_PROMPT.to_string()),
        planner_message(
            "user",
            format!("TRUSTED_PLANNER_CONTEXT\n{}", context_payload),
        ),
    ];

    if input.conversation.is_empty() {
        messages.push(planner_message("user", input.user_message.clone()));
        return Ok(messages);
    }

    for message in &input.conversation {
        messages.push(planner_message(
            planner_conversation_role(message.role),
            planner_conversation_content(message),
        ));
    }
    Ok(messages)
}

fn planner_guidance_contract_payload(
    guidance: &sieve_types::PlannerGuidanceFrame,
) -> Option<PlannerGuidanceContract> {
    let signal = PlannerGuidanceSignal::try_from(guidance.code).ok()?;
    match signal {
        PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch => Some(PlannerGuidanceContract {
            required_action_class: Some("fetch"),
            forbidden_action_classes: Some(vec!["discovery"]),
            require_non_asset_target: Some(true),
            prefer_markdown_view: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueNeedHigherQualitySource => Some(PlannerGuidanceContract {
            required_action_class: Some("fetch"),
            forbidden_action_classes: Some(vec!["discovery"]),
            require_non_asset_target: Some(true),
            require_action_change: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl => Some(PlannerGuidanceContract {
            required_action_class: Some("fetch"),
            require_non_asset_target: Some(true),
            prefer_markdown_view: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueNeedUrlExtraction => Some(PlannerGuidanceContract {
            required_action_class: Some("extract"),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool => {
            Some(PlannerGuidanceContract {
                require_action_change: Some(true),
                ..Default::default()
            })
        }
        PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction => {
            Some(PlannerGuidanceContract {
                forbidden_action_classes: Some(vec!["discovery"]),
                require_action_change: Some(true),
                ..Default::default()
            })
        }
        PlannerGuidanceSignal::ContinueNeedCurrentPageInspection => Some(PlannerGuidanceContract {
            required_action_class: Some("extract"),
            forbidden_action_classes: Some(vec!["discovery"]),
            prefer_current_browser_session: Some(true),
            require_action_change: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial => {
            Some(PlannerGuidanceContract {
                require_action_change: Some(true),
                forbidden_action_classes: Some(vec!["discovery"]),
                avoid_recent_interstitial_origin: Some(true),
                preserve_task_target: Some(true),
                ..Default::default()
            })
        }
        PlannerGuidanceSignal::ContinueNeedCommandReformulation => Some(PlannerGuidanceContract {
            require_action_change: Some(true),
            preserve_task_target: Some(true),
            ..Default::default()
        }),
        _ => None,
    }
}

fn planner_conversation_role(role: sieve_types::PlannerConversationRole) -> &'static str {
    match role {
        sieve_types::PlannerConversationRole::User => "user",
        sieve_types::PlannerConversationRole::Assistant => "assistant",
    }
}

fn planner_conversation_content(message: &sieve_types::PlannerConversationMessage) -> String {
    match message.kind {
        PlannerConversationMessageKind::FullText => message.content.clone(),
        PlannerConversationMessageKind::RedactedInfo => message.content.clone(),
    }
}

fn planner_message(role: &'static str, content: String) -> Value {
    serde_json::to_value(PlannerChatMessage { role, content })
        .expect("planner message serialization should succeed")
}

fn planner_command_catalog_for_allowed_tools(
    allowed_tools: &[String],
) -> Vec<PlannerCommandCatalogEntry<'static>> {
    if !allowed_tools.iter().any(|tool| tool == "bash") {
        return Vec::new();
    }

    planner_command_catalog()
        .iter()
        .map(|descriptor| PlannerCommandCatalogEntry {
            command: descriptor.command,
            description: descriptor.description,
        })
        .collect()
}

fn runtime_event_kind(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::ApprovalRequested(_) => "approval_requested",
        RuntimeEvent::ApprovalResolved(_) => "approval_resolved",
        RuntimeEvent::CodexSessionStatus(_) => "codex_session_status",
        RuntimeEvent::PolicyEvaluated(_) => "policy_evaluated",
        RuntimeEvent::QuarantineCompleted(_) => "quarantine_completed",
        RuntimeEvent::AssistantMessage(_) => "assistant_message",
    }
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
    if raw.len() <= max_chars {
        raw
    } else {
        format!("{}...[truncated]", &raw[..max_chars])
    }
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
