mod automation;
mod codex;
mod provenance;

use crate::{
    make_error, supported_tools, BashArgs, ContractError, ContractErrorParts,
    ToolContractErrorCode, TypedCall, TOOL_AUTOMATION, TOOL_BASH, TOOL_CODEX_EXEC,
    TOOL_CODEX_SESSION, TOOL_DECLASSIFY, TOOL_ENDORSE,
};
use serde_json::{Map, Value};

pub fn validate(tool_name: &str, args_json: &Value) -> Result<TypedCall, ContractError> {
    validate_at_index(0, tool_name, args_json)
}

pub fn validate_at_index(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<TypedCall, ContractError> {
    match tool_name {
        TOOL_AUTOMATION => {
            automation::parse(tool_call_index, tool_name, args_json).map(TypedCall::Automation)
        }
        TOOL_BASH => parse_bash(tool_call_index, tool_name, args_json).map(TypedCall::Bash),
        TOOL_CODEX_EXEC => {
            codex::parse_exec(tool_call_index, tool_name, args_json).map(TypedCall::CodexExec)
        }
        TOOL_CODEX_SESSION => {
            codex::parse_session(tool_call_index, tool_name, args_json).map(TypedCall::CodexSession)
        }
        TOOL_ENDORSE => {
            provenance::parse_endorse(tool_call_index, tool_name, args_json).map(TypedCall::Endorse)
        }
        TOOL_DECLASSIFY => provenance::parse_declassify(tool_call_index, tool_name, args_json)
            .map(TypedCall::Declassify),
        _ => Err(make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::UnknownTool,
                "/",
                format!("unknown tool `{tool_name}`"),
            )
            .expected(format!("one of {}", supported_tools().join(", ")))
            .found(tool_name)
            .hint("use one of ALLOWED_TOOLS"),
        )),
    }
}

fn parse_bash(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<BashArgs, ContractError> {
    let obj = expect_object(tool_call_index, tool_name, args_json)?;
    reject_unknown_fields(tool_call_index, tool_name, obj, &["cmd"])?;
    let cmd = required_string(tool_call_index, tool_name, obj, "cmd")?;
    if cmd.trim().is_empty() {
        return Err(make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/cmd",
                "bash cmd must be non-empty",
            )
            .expected("non-empty string")
            .found("empty string")
            .hint("pass executable shell command text"),
        ));
    }
    Ok(BashArgs { cmd })
}

pub(super) fn require_present<T>(
    tool_call_index: usize,
    tool_name: &str,
    argument_path: &str,
    value: Option<T>,
    message: &str,
) -> Result<(), ContractError> {
    if value.is_some() {
        return Ok(());
    }
    Err(make_error(
        tool_call_index,
        tool_name,
        ContractErrorParts::new(
            ToolContractErrorCode::MissingRequiredField,
            argument_path,
            message,
        )
        .expected("present"),
    ))
}

pub(super) fn expect_object<'a>(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &'a Value,
) -> Result<&'a Map<String, Value>, ContractError> {
    args_json.as_object().ok_or_else(|| {
        make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidType,
                "/",
                "tool args must be a JSON object",
            )
            .expected("object")
            .found(json_type(args_json)),
        )
    })
}

pub(super) fn reject_unknown_fields(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    allowed: &[&str],
) -> Result<(), ContractError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(make_error(
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::UnknownField,
                    format!("/{key}"),
                    format!("unknown field `{key}`"),
                )
                .expected(format!("one of {}", allowed.join(", ")))
                .found(key.clone()),
            ));
        }
    }
    Ok(())
}

pub(super) fn required_string(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<String, ContractError> {
    let value = object.get(field).ok_or_else(|| {
        make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::MissingRequiredField,
                format!("/{field}"),
                format!("missing required field `{field}`"),
            )
            .expected("string"),
        )
    })?;

    let value = value.as_str().ok_or_else(|| {
        make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidType,
                format!("/{field}"),
                format!("field `{field}` must be a string"),
            )
            .expected("string")
            .found(json_type(value)),
        )
    })?;

    Ok(value.to_string())
}

pub(super) fn optional_string(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<String>, ContractError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };

    if value.is_null() {
        return Ok(None);
    }

    let value = value.as_str().ok_or_else(|| {
        make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidType,
                format!("/{field}"),
                format!("field `{field}` must be string or null"),
            )
            .expected("string|null")
            .found(json_type(value)),
        )
    })?;

    Ok(Some(value.to_string()))
}

pub(super) fn optional_trimmed_string(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<String>, ContractError> {
    Ok(optional_string(tool_call_index, tool_name, object, field)?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

pub(super) fn optional_string_array(
    tool_call_index: usize,
    tool_name: &str,
    obj: &Map<String, Value>,
    field_name: &str,
) -> Result<Vec<String>, ContractError> {
    let Some(value) = obj.get(field_name) else {
        return Ok(Vec::new());
    };
    let array = match value {
        Value::Array(array) => array,
        other => {
            return Err(make_error(
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::InvalidType,
                    format!("/{field_name}"),
                    format!("{field_name} must be an array"),
                )
                .expected("array")
                .found(json_type(other)),
            ))
        }
    };
    let mut out = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let item = value.as_str().ok_or_else(|| {
            make_error(
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::InvalidType,
                    format!("/{field_name}/{index}"),
                    format!("{field_name} entries must be strings"),
                )
                .expected("string")
                .found(json_type(value)),
            )
        })?;
        let trimmed = item.trim();
        if trimmed.is_empty() {
            return Err(make_error(
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::InvalidValue,
                    format!("/{field_name}/{index}"),
                    format!("{field_name} entries must be non-empty"),
                )
                .expected("non-empty string")
                .found("empty string"),
            ));
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

pub(super) fn required_string_array(
    tool_call_index: usize,
    tool_name: &str,
    obj: &Map<String, Value>,
    field_name: &str,
) -> Result<Vec<String>, ContractError> {
    if !obj.contains_key(field_name) {
        return Err(make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::MissingRequiredField,
                format!("/{field_name}"),
                format!("missing required field `{field_name}`"),
            )
            .expected("array"),
        ));
    }
    optional_string_array(tool_call_index, tool_name, obj, field_name)
}

pub(super) fn optional_u64(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, ContractError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value.as_u64().ok_or_else(|| {
        make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidType,
                format!("/{field}"),
                format!("field `{field}` must be integer or null"),
            )
            .expected("integer|null")
            .found(json_type(value)),
        )
    })?;
    Ok(Some(number))
}

pub(super) fn json_type(value: &Value) -> &'static str {
    if value.is_null() {
        "null"
    } else if value.is_boolean() {
        "boolean"
    } else if value.is_number() {
        "number"
    } else if value.is_string() {
        "string"
    } else if value.is_array() {
        "array"
    } else {
        "object"
    }
}
