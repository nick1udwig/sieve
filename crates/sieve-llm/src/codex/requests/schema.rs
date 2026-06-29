use super::types::CodexToolDefinition;
use crate::LlmError;
use serde_json::Value;
use sieve_tool_contracts::tool_args_schema;

pub(super) fn planner_tool_definitions(
    allowed_tools: &[String],
) -> Result<Vec<CodexToolDefinition>, LlmError> {
    let mut tools = Vec::with_capacity(allowed_tools.len());
    for tool_name in allowed_tools {
        let schema = tool_args_schema(tool_name).ok_or_else(|| {
            LlmError::Boundary(format!(
                "allowed tool `{tool_name}` is missing a contract schema"
            ))
        })?;
        let schema = normalize_codex_tool_schema(schema);
        tools.push(CodexToolDefinition {
            tool_type: "function",
            name: tool_name.clone(),
            parameters: schema,
            strict: true,
        });
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
            items
                .iter()
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
