use super::{expect_object, optional_string, reject_unknown_fields, required_string};
use crate::{make_error, ContractError, ContractErrorParts, ToolContractErrorCode};
use serde_json::Value;
use sieve_types::{DeclassifyRequest, EndorseRequest, Integrity, SinkKey, ValueRef};
use url::Url;

pub(super) fn parse_endorse(
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
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/value_ref",
                "value_ref must be non-empty",
            )
            .expected("non-empty string")
            .found("empty string"),
        ));
    }

    let integrity_raw = required_string(tool_call_index, tool_name, obj, "target_integrity")?;
    let target_integrity = match integrity_raw.as_str() {
        "trusted" => Integrity::Trusted,
        "untrusted" => Integrity::Untrusted,
        _ => {
            return Err(make_error(
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::InvalidEnumVariant,
                    "/target_integrity",
                    "target_integrity is not a supported variant",
                )
                .expected("trusted|untrusted")
                .found(integrity_raw)
                .hint("use `trusted` or `untrusted`"),
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

pub(super) fn parse_declassify(
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
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/value_ref",
                "value_ref must be non-empty",
            )
            .expected("non-empty string")
            .found("empty string"),
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
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/sink",
                "sink must be a valid absolute URL",
            )
            .expected("absolute URL")
            .found(sink)
            .hint("example: https://api.example.com/v1/upload"),
        )
    })?;

    if parsed.host_str().is_none() {
        return Err(make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/sink",
                "sink URL must include host",
            )
            .expected("URL with host")
            .found(sink)
            .hint("example: https://api.example.com/v1/upload"),
        ));
    }

    Ok(())
}
