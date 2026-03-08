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
        "stream": true,
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
        "stream": true,
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
        let schema = normalize_codex_tool_schema(schema);
        tools.push(json!({
            "type": "function",
            "name": tool_name,
            "parameters": schema,
            "strict": true
        }));
    }
    Ok(tools)
}

fn normalize_codex_tool_schema(schema: Value) -> Value {
    match schema {
        Value::Object(mut map) => {
            if let Some(one_of) = map.remove("oneOf") {
                return normalize_one_of_schema(one_of, map);
            }

            let mut normalized: serde_json::Map<String, Value> = map
                .into_iter()
                .map(|(key, value)| (key, normalize_codex_tool_schema(value)))
                .collect();
            codex_require_all_object_properties(&mut normalized);
            Value::Object(normalized)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(normalize_codex_tool_schema)
                .collect(),
        ),
        other => other,
    }
}

fn normalize_one_of_schema(
    one_of: Value,
    mut surrounding: serde_json::Map<String, Value>,
) -> Value {
    let Some(variants) = one_of.as_array() else {
        surrounding.insert(
            "description".to_string(),
            Value::String("oneOf".to_string()),
        );
        return Value::Object(surrounding);
    };

    let mut object_variants = Vec::new();
    let mut has_null_variant = false;
    for variant in variants {
        match variant {
            Value::Object(obj) if obj.get("type").and_then(Value::as_str) == Some("object") => {
                object_variants.push(obj.clone());
            }
            Value::Object(obj) if obj.get("type").and_then(Value::as_str) == Some("null") => {
                has_null_variant = true;
            }
            Value::Null => has_null_variant = true,
            _ => return Value::Object(surrounding),
        }
    }

    if object_variants.is_empty() {
        return if has_null_variant {
            Value::Object(serde_json::Map::from_iter([(
                "type".to_string(),
                Value::Array(vec![
                    Value::String("null".to_string()),
                    Value::String("object".to_string()),
                ]),
            )]))
        } else {
            Value::Object(surrounding)
        };
    }

    let mut merged_properties = serde_json::Map::new();
    let mut shared_required: Option<Vec<String>> = None;
    let mut variant_notes = Vec::new();

    for variant in &object_variants {
        let kind_note = variant
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|props| props.get("kind"))
            .and_then(Value::as_object)
            .and_then(|kind| kind.get("const"))
            .and_then(Value::as_str)
            .unwrap_or("variant")
            .to_string();
        let required = variant
            .get("required")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !required.is_empty() {
            variant_notes.push(format!("{kind_note}: requires {}", required.join(", ")));
        }
        shared_required = Some(match shared_required.take() {
            None => required.clone(),
            Some(existing) => existing
                .into_iter()
                .filter(|item| required.iter().any(|candidate| candidate == item))
                .collect(),
        });

        if let Some(props) = variant.get("properties").and_then(Value::as_object) {
            for (name, value) in props {
                match merged_properties.get_mut(name) {
                    Some(existing) => merge_property_schema(existing, value),
                    None => {
                        merged_properties
                            .insert(name.clone(), normalize_codex_tool_schema(value.clone()));
                    }
                }
            }
        }
    }

    let mut out = serde_json::Map::new();
    out.insert(
        "type".to_string(),
        if has_null_variant {
            Value::Array(vec![
                Value::String("object".to_string()),
                Value::String("null".to_string()),
            ])
        } else {
            Value::String("object".to_string())
        },
    );
    out.insert("additionalProperties".to_string(), Value::Bool(false));
    let required = if has_null_variant {
        merged_properties.keys().cloned().collect::<Vec<_>>()
    } else {
        shared_required.unwrap_or_default()
    };
    if has_null_variant {
        for value in merged_properties.values_mut() {
            make_schema_nullable(value);
        }
    }
    out.insert("properties".to_string(), Value::Object(merged_properties));
    if !required.is_empty() {
        out.insert(
            "required".to_string(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }

    let mut notes = variant_notes;
    if has_null_variant {
        notes.push("null allowed".to_string());
    }
    if !notes.is_empty() {
        out.insert("description".to_string(), Value::String(notes.join("; ")));
    }
    Value::Object(out)
}

fn merge_property_schema(existing: &mut Value, incoming: &Value) {
    let normalized_incoming = normalize_codex_tool_schema(incoming.clone());
    let Some(existing_obj) = existing.as_object_mut() else {
        *existing = normalized_incoming;
        return;
    };
    let Some(incoming_obj) = normalized_incoming.as_object() else {
        return;
    };

    let existing_const = existing_obj
        .get("const")
        .and_then(Value::as_str)
        .map(str::to_string);
    let incoming_const = incoming_obj
        .get("const")
        .and_then(Value::as_str)
        .map(str::to_string);

    match (existing_const.as_deref(), incoming_const.as_deref()) {
        (Some(left), Some(right)) if left != right => {
            existing_obj.remove("const");
            existing_obj.insert("type".to_string(), Value::String("string".to_string()));
            existing_obj.insert(
                "enum".to_string(),
                Value::Array(vec![
                    Value::String(left.to_string()),
                    Value::String(right.to_string()),
                ]),
            );
        }
        _ => {}
    }

    if let (Some(existing_enum), Some(incoming_const)) = (
        existing_obj.get_mut("enum").and_then(Value::as_array_mut),
        incoming_const.as_deref(),
    ) {
        let already_present = existing_enum
            .iter()
            .filter_map(Value::as_str)
            .any(|candidate| candidate == incoming_const);
        if !already_present {
            existing_enum.push(Value::String(incoming_const.to_string()));
        }
    }
}

fn make_schema_nullable(schema: &mut Value) {
    let Some(obj) = schema.as_object_mut() else {
        return;
    };
    match obj.get("type") {
        Some(Value::String(single)) if single != "null" => {
            obj.insert(
                "type".to_string(),
                Value::Array(vec![
                    Value::String(single.clone()),
                    Value::String("null".to_string()),
                ]),
            );
        }
        Some(Value::Array(types)) => {
            let has_null = types.iter().any(|value| value.as_str() == Some("null"));
            if !has_null {
                let mut next = types.clone();
                next.push(Value::String("null".to_string()));
                obj.insert("type".to_string(), Value::Array(next));
            }
        }
        _ => {}
    }
}

fn codex_require_all_object_properties(map: &mut serde_json::Map<String, Value>) {
    let is_object = match map.get("type") {
        Some(Value::String(kind)) => kind == "object",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind.as_str() == Some("object")),
        _ => false,
    };
    if !is_object {
        return;
    }
    let property_names = map
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>());
    let Some(property_names) = property_names else {
        return;
    };
    let existing_required = map
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    for name in &property_names {
        let already_required = existing_required.iter().any(|required| required == name);
        if !already_required {
            if let Some(schema) = properties.get_mut(name) {
                make_schema_nullable(schema);
            }
        }
    }

    map.insert(
        "required".to_string(),
        Value::Array(property_names.into_iter().map(Value::String).collect()),
    );
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
    fn planner_tool_definitions_flatten_codex_incompatible_one_of() {
        let tools = planner_tool_definitions(&["automation".to_string()]).expect("tools");
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
