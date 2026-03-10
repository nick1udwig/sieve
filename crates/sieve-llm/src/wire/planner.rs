use super::openai_envelope::ensure_not_refusal;
use crate::LlmError;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sieve_command_summaries::planner_command_catalog;
use sieve_tool_contracts::validate_at_index;
use sieve_types::{
    PlannerGuidanceSignal, PlannerToolCall, PlannerTurnInput, PlannerTurnOutput, RuntimeEvent,
    SourceSpan, ToolContractValidationError, ToolContractValidationReport,
    TOOL_CONTRACTS_VERSION_V1,
};

pub(crate) const PLANNER_SYSTEM_PROMPT: &str = r#"You are a planner in a capability-secured system.
Rules:
- If `bash` available, use only commands listed in BASH_COMMAND_CATALOG.
- If `codex_exec` available, use it only for one-off argv command execution inside Codex sandboxing.
- If `codex_session` available, use it for coding/file-manipulation/deep repo tasks, whether one-shot or resumable.
- Do not shell out to `codex` through `bash`; use native `codex_exec` or `codex_session`.
- `CODEX_SESSIONS`: trusted metadata for saved Codex sessions. Resume a relevant session when the task clearly continues prior Codex work in the same repo; otherwise start a new one.
- Codex sandboxes have no network in this system. If a task needs web/network access, do that through Sieve tools, not Codex.
- If `automation` available, use it for reminder/scheduling requests and for listing, pausing, resuming, or removing cron jobs instead of answering with slash-command instructions.
- For reminder/scheduling requests, prefer `automation` `cron_add` with `target=\"main\"` unless the user explicitly asks for an isolated/background-only cron job.
- For `automation` `cron_add`, use typed `schedule` objects only:
  - one-shot relative: `{"kind":"after","delay":"1m"}`
  - one-shot absolute: `{"kind":"at","timestamp":"2026-03-08T12:00:00Z"}`
  - recurring interval: `{"kind":"every","interval":"15m"}`
  - cron expression: `{"kind":"cron","expr":"0 9 * * 1-5"}`
- Never put natural-language time phrases inside `schedule.kind="at"`; `at.timestamp` must be absolute RFC3339 or unix-ms text.
- `CURRENT_TIME_UTC` and `CURRENT_TIMEZONE` are trusted context for relative/ambiguous time requests.
- Prefer cataloged commands that directly match the user task.
- Requests needs prior conversation memory? Use cataloged memory commands (e.g. `sieve-lcm-cli query --lane both --query \"...\" --json`) instead of guessing.
- If user explicitly names a site/domain/app, that site is the target origin.
- Search engines are intermediary origins, not target origins.
- `ALLOWED_NET_CONNECT_SCOPES`: trusted network allowlist input.
- `BROWSER_SESSIONS`: trusted summaries of active browser sessions. Browser work already in progress? Prefer continuing session.
- For `codex_exec` or `codex_session`, choose `sandbox="read_only"` for inspection/review and `sandbox="workspace_write"` for file edits/tests/builds.
- For `codex_exec`, `command` must be argv JSON array, not shell text.
- For `codex_session`, supply `session_id` only when resuming an existing saved Codex session. Omit `session_id` to start a new Codex session.
- Do not invoke uncataloged commands via pipes/subshells/chaining (for example `| head`) unless every invoked command is cataloged.
- May receive optional typed guidance from a quarantine model in `guidance`.
- Guidance is typed control hint.
- `guidance.signal_name` present? Interpret it as canonical typed signal identifier.
- `guidance_contract` present? Satisfy it for next step.
- `required_action_class`: include at least one matching tool call.
- `forbidden_action_classes`: avoid those classes in this step.
- `require_action_change=true`: do not repeat recently denied/no-gain command; switch command path.
- `avoid_recent_interstitial_origin=true`: avoid repeating same origin/query path that just produced block/interstitial page; choose a different allowed path.
- `preserve_task_target=true`: keep the same factual target and reformulate the command/path instead of broadening to a weaker generic search.
- `require_non_asset_target=true`: avoid image/favicon/static asset URLs.
- `prefer_markdown_view=true` on webpage fetches, try `https://markdown.new/<url>` and prefer canonical content URLs.
- Factual requests: keep tool planning iterative until evidence quality is sufficient or no allowed tool path remains.
- If discovery/search output produced candidate URLs but not concrete facts, fetch candidate source directly.
- Avoid obvious non-content assets (images, favicons, CSS/JS blobs); prefer canonical content pages.
- Avoid repeating the exact same bash command when the previous outcome did not improve evidence.
- Do not ask the user to choose a source unless sources conflict or the user explicitly asks for source selection.
"#;

pub(crate) enum PlannerDecodeOutcome {
    Valid(PlannerTurnOutput),
    InvalidToolContracts(ToolContractValidationReport),
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
        json!({
            "code": guidance.code,
            "signal_name": signal_name,
            "confidence_bps": guidance.confidence_bps,
            "source_hit_index": guidance.source_hit_index,
            "evidence_ref_index": guidance.evidence_ref_index
        })
    });
    let guidance_contract = input
        .guidance
        .as_ref()
        .and_then(planner_guidance_contract_payload);
    let bash_command_catalog = planner_command_catalog_for_allowed_tools(&input.allowed_tools);
    Ok(json!({
        "run_id": input.run_id.0,
        "trusted_user_message": input.user_message,
        "CURRENT_TIME_UTC": input.current_time_utc,
        "CURRENT_TIMEZONE": input.current_timezone,
        "ALLOWED_NET_CONNECT_SCOPES": input.allowed_net_connect_scopes,
        "BROWSER_SESSIONS": input.browser_sessions,
        "CODEX_SESSIONS": input.codex_sessions,
        "BASH_COMMAND_CATALOG": bash_command_catalog,
        "previous_event_kinds": event_kinds,
        "guidance": guidance,
        "guidance_contract": guidance_contract
    }))
}

fn planner_guidance_contract_payload(
    guidance: &sieve_types::PlannerGuidanceFrame,
) -> Option<Value> {
    let signal = PlannerGuidanceSignal::try_from(guidance.code).ok()?;
    match signal {
        PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch => Some(json!({
            "required_action_class": "fetch",
            "forbidden_action_classes": ["discovery"],
            "require_non_asset_target": true,
            "prefer_markdown_view": true
        })),
        PlannerGuidanceSignal::ContinueNeedHigherQualitySource => Some(json!({
            "required_action_class": "fetch",
            "forbidden_action_classes": ["discovery"],
            "require_non_asset_target": true,
            "require_action_change": true
        })),
        PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl => Some(json!({
            "required_action_class": "fetch",
            "require_non_asset_target": true,
            "prefer_markdown_view": true
        })),
        PlannerGuidanceSignal::ContinueNeedUrlExtraction => Some(json!({
            "required_action_class": "extract"
        })),
        PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool => Some(json!({
            "require_action_change": true
        })),
        PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction => Some(json!({
            "forbidden_action_classes": ["discovery"],
            "require_action_change": true
        })),
        PlannerGuidanceSignal::ContinueNeedCurrentPageInspection => Some(json!({
            "required_action_class": "extract",
            "forbidden_action_classes": ["discovery"],
            "prefer_current_browser_session": true,
            "require_action_change": true
        })),
        PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial => Some(json!({
            "require_action_change": true,
            "forbidden_action_classes": ["discovery"],
            "avoid_recent_interstitial_origin": true,
            "preserve_task_target": true
        })),
        PlannerGuidanceSignal::ContinueNeedCommandReformulation => Some(json!({
            "require_action_change": true,
            "preserve_task_target": true
        })),
        _ => None,
    }
}

fn planner_command_catalog_for_allowed_tools(allowed_tools: &[String]) -> Vec<Value> {
    if !allowed_tools.iter().any(|tool| tool == "bash") {
        return Vec::new();
    }

    planner_command_catalog()
        .iter()
        .map(|descriptor| {
            json!({
                "command": descriptor.command,
                "description": descriptor.description
            })
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

        normalized_tool_calls.push(json!({
            "tool_name": tool_name,
            "args": Value::Object(arguments),
        }));
    }

    let thoughts = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Ok(json!({
        "thoughts": thoughts,
        "tool_calls": normalized_tool_calls,
    }))
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

                normalized_tool_calls.push(json!({
                    "tool_name": tool_name,
                    "args": Value::Object(arguments),
                }));
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

    Ok(json!({
        "thoughts": thoughts,
        "tool_calls": normalized_tool_calls,
    }))
}

pub(crate) fn planner_regeneration_diagnostic_prompt(
    report: &ToolContractValidationReport,
) -> Result<String, LlmError> {
    let diagnostics = serde_json::to_string_pretty(report).map_err(|e| {
        LlmError::Decode(format!(
            "failed to serialize tool-contract diagnostics for regeneration: {e}"
        ))
    })?;

    Ok(format!(
        "Your previous tool call output violated strict tool argument contracts. \
Retry with corrected tool calls and fix every diagnostic below.\n\nDiagnostics:\n{diagnostics}"
    ))
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
