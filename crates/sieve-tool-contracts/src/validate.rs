use crate::{
    make_error, supported_tools, BashArgs, ContractError, ToolContractErrorCode, TypedCall,
    TOOL_BASH, TOOL_DECLASSIFY, TOOL_ENDORSE,
};
use serde_json::{Map, Value};
use sieve_types::{DeclassifyRequest, EndorseRequest, Integrity, SinkKey, ValueRef};
use url::Url;

pub fn validate(tool_name: &str, args_json: &Value) -> Result<TypedCall, ContractError> {
    validate_at_index(0, tool_name, args_json)
}

pub fn validate_at_index(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<TypedCall, ContractError> {
    match tool_name {
        TOOL_BASH => parse_bash(tool_call_index, tool_name, args_json).map(TypedCall::Bash),
        TOOL_ENDORSE => {
            parse_endorse(tool_call_index, tool_name, args_json).map(TypedCall::Endorse)
        }
        TOOL_DECLASSIFY => {
            parse_declassify(tool_call_index, tool_name, args_json).map(TypedCall::Declassify)
        }
        _ => Err(make_error(
            ToolContractErrorCode::UnknownTool,
            tool_call_index,
            tool_name,
            "/",
            Some(format!("one of {}", supported_tools().join(", "))),
            Some(tool_name.to_string()),
            format!("unknown tool `{tool_name}`"),
            Some("use one of ALLOWED_TOOLS"),
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
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/cmd",
            Some("non-empty string".to_string()),
            Some("empty string".to_string()),
            "bash cmd must be non-empty".to_string(),
            Some("pass executable shell command text"),
        ));
    }
    Ok(BashArgs { cmd })
}

fn parse_endorse(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<EndorseRequest, ContractError> {
    let obj = expect_object(tool_call_index, tool_name, args_json)?;
    reject_unknown_fields(
        tool_call_index,
        tool_name,
        obj,
        &["value_ref", "target_integrity", "reason"],
    )?;

    let value_ref = required_string(tool_call_index, tool_name, obj, "value_ref")?;
    if value_ref.trim().is_empty() {
        return Err(make_error(
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/value_ref",
            Some("non-empty string".to_string()),
            Some("empty string".to_string()),
            "value_ref must be non-empty".to_string(),
            None,
        ));
    }

    let integrity_raw = required_string(tool_call_index, tool_name, obj, "target_integrity")?;
    let target_integrity = match integrity_raw.as_str() {
        "trusted" => Integrity::Trusted,
        "untrusted" => Integrity::Untrusted,
        _ => {
            return Err(make_error(
                ToolContractErrorCode::InvalidEnumVariant,
                tool_call_index,
                tool_name,
                "/target_integrity",
                Some("trusted|untrusted".to_string()),
                Some(integrity_raw),
                "target_integrity is not a supported variant".to_string(),
                Some("use `trusted` or `untrusted`"),
            ))
        }
    };

    let reason = optional_string(tool_call_index, tool_name, obj, "reason")?;
    Ok(EndorseRequest {
        value_ref: ValueRef(value_ref),
        target_integrity,
        reason,
    })
}

fn parse_declassify(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<DeclassifyRequest, ContractError> {
    let obj = expect_object(tool_call_index, tool_name, args_json)?;
    reject_unknown_fields(
        tool_call_index,
        tool_name,
        obj,
        &["value_ref", "sink", "reason"],
    )?;

    let value_ref = required_string(tool_call_index, tool_name, obj, "value_ref")?;
    if value_ref.trim().is_empty() {
        return Err(make_error(
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/value_ref",
            Some("non-empty string".to_string()),
            Some("empty string".to_string()),
            "value_ref must be non-empty".to_string(),
            None,
        ));
    }

    let sink = required_string(tool_call_index, tool_name, obj, "sink")?;
    validate_sink(tool_call_index, tool_name, &sink)?;

    let reason = optional_string(tool_call_index, tool_name, obj, "reason")?;
    Ok(DeclassifyRequest {
        value_ref: ValueRef(value_ref),
        sink: SinkKey(sink),
        reason,
    })
}

fn validate_sink(tool_call_index: usize, tool_name: &str, sink: &str) -> Result<(), ContractError> {
    let parsed = Url::parse(sink).map_err(|_| {
        make_error(
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/sink",
            Some("absolute URL".to_string()),
            Some(sink.to_string()),
            "sink must be a valid absolute URL".to_string(),
            Some("example: https://api.example.com/v1/upload"),
        )
    })?;

    if parsed.host_str().is_none() {
        return Err(make_error(
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/sink",
            Some("URL with host".to_string()),
            Some(sink.to_string()),
            "sink URL must include host".to_string(),
            Some("example: https://api.example.com/v1/upload"),
        ));
    }

    Ok(())
}

fn expect_object<'a>(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &'a Value,
) -> Result<&'a Map<String, Value>, ContractError> {
    args_json.as_object().ok_or_else(|| {
        make_error(
            ToolContractErrorCode::InvalidType,
            tool_call_index,
            tool_name,
            "/",
            Some("object".to_string()),
            Some(json_type(args_json).to_string()),
            "tool args must be a JSON object".to_string(),
            None,
        )
    })
}

fn reject_unknown_fields(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    allowed: &[&str],
) -> Result<(), ContractError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(make_error(
                ToolContractErrorCode::UnknownField,
                tool_call_index,
                tool_name,
                &format!("/{key}"),
                Some(format!("one of {}", allowed.join(", "))),
                Some(key.clone()),
                format!("unknown field `{key}`"),
                None,
            ));
        }
    }
    Ok(())
}

fn required_string(
    tool_call_index: usize,
    tool_name: &str,
    object: &Map<String, Value>,
    field: &str,
) -> Result<String, ContractError> {
    let value = object.get(field).ok_or_else(|| {
        make_error(
            ToolContractErrorCode::MissingRequiredField,
            tool_call_index,
            tool_name,
            &format!("/{field}"),
            Some("string".to_string()),
            None,
            format!("missing required field `{field}`"),
            None,
        )
    })?;

    let value = value.as_str().ok_or_else(|| {
        make_error(
            ToolContractErrorCode::InvalidType,
            tool_call_index,
            tool_name,
            &format!("/{field}"),
            Some("string".to_string()),
            Some(json_type(value).to_string()),
            format!("field `{field}` must be a string"),
            None,
        )
    })?;

    Ok(value.to_string())
}

fn optional_string(
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
            ToolContractErrorCode::InvalidType,
            tool_call_index,
            tool_name,
            &format!("/{field}"),
            Some("string|null".to_string()),
            Some(json_type(value).to_string()),
            format!("field `{field}` must be string or null"),
            None,
        )
    })?;

    Ok(Some(value.to_string()))
}

fn json_type(value: &Value) -> &'static str {
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
