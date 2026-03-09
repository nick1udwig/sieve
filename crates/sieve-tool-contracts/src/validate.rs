use crate::{
    make_error, supported_tools, AutomationArgs, BashArgs, CodexExecArgs, CodexSessionArgs,
    ContractError, ToolContractErrorCode, TypedCall, TOOL_AUTOMATION, TOOL_BASH, TOOL_CODEX_EXEC,
    TOOL_CODEX_SESSION, TOOL_DECLASSIFY, TOOL_ENDORSE,
};
use serde_json::{Map, Value};
use sieve_types::{
    AutomationAction, AutomationRequest, AutomationSchedule, AutomationTarget, CodexExecRequest,
    CodexSandboxMode, CodexSessionRequest, DeclassifyRequest, EndorseRequest, Integrity, SinkKey,
    ValueRef,
};
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
        TOOL_AUTOMATION => {
            parse_automation(tool_call_index, tool_name, args_json).map(TypedCall::Automation)
        }
        TOOL_BASH => parse_bash(tool_call_index, tool_name, args_json).map(TypedCall::Bash),
        TOOL_CODEX_EXEC => {
            parse_codex_exec(tool_call_index, tool_name, args_json).map(TypedCall::CodexExec)
        }
        TOOL_CODEX_SESSION => {
            parse_codex_session(tool_call_index, tool_name, args_json).map(TypedCall::CodexSession)
        }
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

fn parse_automation(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<AutomationRequest, ContractError> {
    let obj = expect_object(tool_call_index, tool_name, args_json)?;
    reject_unknown_fields(
        tool_call_index,
        tool_name,
        obj,
        &["action", "target", "schedule", "prompt", "job_id"],
    )?;
    let args = AutomationArgs {
        action: required_string(tool_call_index, tool_name, obj, "action")?,
        target: optional_string(tool_call_index, tool_name, obj, "target")?,
        schedule: obj.get("schedule").cloned(),
        prompt: optional_string(tool_call_index, tool_name, obj, "prompt")?,
        job_id: optional_string(tool_call_index, tool_name, obj, "job_id")?,
    };

    let action = match args.action.as_str() {
        "cron_list" => AutomationAction::CronList,
        "cron_add" => AutomationAction::CronAdd,
        "cron_remove" => AutomationAction::CronRemove,
        "cron_pause" => AutomationAction::CronPause,
        "cron_resume" => AutomationAction::CronResume,
        _ => {
            return Err(make_error(
                ToolContractErrorCode::InvalidEnumVariant,
                tool_call_index,
                tool_name,
                "/action",
                Some("cron_list|cron_add|cron_remove|cron_pause|cron_resume".to_string()),
                Some(args.action),
                "action is not a supported variant".to_string(),
                Some("use a supported automation action"),
            ))
        }
    };

    let target = match args.target {
        Some(target) => Some(match target.as_str() {
            "main" => AutomationTarget::Main,
            "isolated" => AutomationTarget::Isolated,
            _ => {
                return Err(make_error(
                    ToolContractErrorCode::InvalidEnumVariant,
                    tool_call_index,
                    tool_name,
                    "/target",
                    Some("main|isolated".to_string()),
                    Some(target),
                    "target is not a supported variant".to_string(),
                    Some("use `main` or `isolated`"),
                ))
            }
        }),
        None => None,
    };

    let schedule = match args.schedule {
        Some(raw) => Some(parse_automation_schedule(tool_call_index, tool_name, &raw)?),
        None => None,
    };

    match action {
        AutomationAction::CronList => {}
        AutomationAction::CronAdd => {
            require_present(
                tool_call_index,
                tool_name,
                "/target",
                target.as_ref().map(|_| "ok"),
                "cron_add requires target",
            )?;
            require_present(
                tool_call_index,
                tool_name,
                "/schedule",
                schedule.as_ref().map(|_| "ok"),
                "cron_add requires schedule",
            )?;
            require_present(
                tool_call_index,
                tool_name,
                "/prompt",
                args.prompt.as_deref(),
                "cron_add requires prompt",
            )?;
        }
        AutomationAction::CronRemove
        | AutomationAction::CronPause
        | AutomationAction::CronResume => {
            require_present(
                tool_call_index,
                tool_name,
                "/job_id",
                args.job_id.as_deref(),
                "cron action requires job_id",
            )?;
        }
    }

    Ok(AutomationRequest {
        action,
        target,
        schedule,
        prompt: args.prompt,
        job_id: args.job_id,
    })
}

fn parse_automation_schedule(
    tool_call_index: usize,
    tool_name: &str,
    value: &Value,
) -> Result<AutomationSchedule, ContractError> {
    let obj = match value {
        Value::Object(obj) => obj,
        other => {
            return Err(make_error(
                ToolContractErrorCode::InvalidType,
                tool_call_index,
                tool_name,
                "/schedule",
                Some("object".to_string()),
                Some(json_type(other).to_string()),
                "schedule must be an object".to_string(),
                Some("use a typed schedule object"),
            ))
        }
    };
    reject_unknown_fields(
        tool_call_index,
        tool_name,
        obj,
        &["kind", "delay", "timestamp", "interval", "expr"],
    )?;
    let kind = required_string(tool_call_index, tool_name, obj, "kind")?;
    match kind.as_str() {
        "after" => Ok(AutomationSchedule::After {
            delay: required_string(tool_call_index, tool_name, obj, "delay")?,
        }),
        "at" => Ok(AutomationSchedule::At {
            timestamp: required_string(tool_call_index, tool_name, obj, "timestamp")?,
        }),
        "every" => Ok(AutomationSchedule::Every {
            interval: required_string(tool_call_index, tool_name, obj, "interval")?,
        }),
        "cron" => Ok(AutomationSchedule::Cron {
            expr: required_string(tool_call_index, tool_name, obj, "expr")?,
        }),
        _ => Err(make_error(
            ToolContractErrorCode::InvalidEnumVariant,
            tool_call_index,
            tool_name,
            "/schedule/kind",
            Some("after|at|every|cron".to_string()),
            Some(kind),
            "schedule kind is not a supported variant".to_string(),
            Some("use `after`, `at`, `every`, or `cron`"),
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

fn parse_codex_exec(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<CodexExecRequest, ContractError> {
    let obj = expect_object(tool_call_index, tool_name, args_json)?;
    reject_unknown_fields(
        tool_call_index,
        tool_name,
        obj,
        &["command", "sandbox", "cwd", "writable_roots", "timeout_ms"],
    )?;
    let args = CodexExecArgs {
        command: required_string_array(tool_call_index, tool_name, obj, "command")?,
        sandbox: required_string(tool_call_index, tool_name, obj, "sandbox")?,
        cwd: optional_string(tool_call_index, tool_name, obj, "cwd")?,
        writable_roots: optional_string_array(tool_call_index, tool_name, obj, "writable_roots")?,
        timeout_ms: optional_u64(tool_call_index, tool_name, obj, "timeout_ms")?,
    };
    Ok(CodexExecRequest {
        command: non_empty_command(tool_call_index, tool_name, &args.command)?,
        sandbox: parse_codex_sandbox(tool_call_index, tool_name, &args.sandbox, "/sandbox")?,
        cwd: args.cwd,
        writable_roots: args.writable_roots,
        timeout_ms: args.timeout_ms,
    })
}

fn parse_codex_session(
    tool_call_index: usize,
    tool_name: &str,
    args_json: &Value,
) -> Result<CodexSessionRequest, ContractError> {
    let obj = expect_object(tool_call_index, tool_name, args_json)?;
    reject_unknown_fields(
        tool_call_index,
        tool_name,
        obj,
        &[
            "session_id",
            "instruction",
            "sandbox",
            "cwd",
            "writable_roots",
            "local_images",
        ],
    )?;
    let args = CodexSessionArgs {
        session_id: optional_string(tool_call_index, tool_name, obj, "session_id")?,
        instruction: required_string(tool_call_index, tool_name, obj, "instruction")?,
        sandbox: required_string(tool_call_index, tool_name, obj, "sandbox")?,
        cwd: optional_string(tool_call_index, tool_name, obj, "cwd")?,
        writable_roots: optional_string_array(tool_call_index, tool_name, obj, "writable_roots")?,
        local_images: optional_string_array(tool_call_index, tool_name, obj, "local_images")?,
    };
    Ok(CodexSessionRequest {
        session_id: args.session_id,
        instruction: non_empty_instruction(tool_call_index, tool_name, &args.instruction)?,
        sandbox: parse_codex_sandbox(tool_call_index, tool_name, &args.sandbox, "/sandbox")?,
        cwd: args.cwd,
        writable_roots: args.writable_roots,
        local_images: args.local_images,
    })
}

fn non_empty_instruction(
    tool_call_index: usize,
    tool_name: &str,
    instruction: &str,
) -> Result<String, ContractError> {
    let trimmed = instruction.trim();
    if trimmed.is_empty() {
        return Err(make_error(
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/instruction",
            Some("non-empty string".to_string()),
            Some("empty string".to_string()),
            "instruction must be non-empty".to_string(),
            Some("pass task instructions for Codex"),
        ));
    }
    Ok(trimmed.to_string())
}

fn non_empty_command(
    tool_call_index: usize,
    tool_name: &str,
    command: &[String],
) -> Result<Vec<String>, ContractError> {
    if command.is_empty() {
        return Err(make_error(
            ToolContractErrorCode::InvalidValue,
            tool_call_index,
            tool_name,
            "/command",
            Some("non-empty array".to_string()),
            Some("empty array".to_string()),
            "command must include at least one argv entry".to_string(),
            Some("pass command as argv array, for example [\"git\",\"status\"]"),
        ));
    }
    Ok(command.to_vec())
}

fn parse_codex_sandbox(
    tool_call_index: usize,
    tool_name: &str,
    raw: &str,
    argument_path: &str,
) -> Result<CodexSandboxMode, ContractError> {
    match raw {
        "read_only" => Ok(CodexSandboxMode::ReadOnly),
        "workspace_write" => Ok(CodexSandboxMode::WorkspaceWrite),
        _ => Err(make_error(
            ToolContractErrorCode::InvalidEnumVariant,
            tool_call_index,
            tool_name,
            argument_path,
            Some("read_only|workspace_write".to_string()),
            Some(raw.to_string()),
            "sandbox is not a supported variant".to_string(),
            Some("use `read_only` or `workspace_write`"),
        )),
    }
}

fn require_present<T>(
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
        ToolContractErrorCode::MissingRequiredField,
        tool_call_index,
        tool_name,
        argument_path,
        Some("present".to_string()),
        None,
        message.to_string(),
        None,
    ))
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

fn optional_string_array(
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
                ToolContractErrorCode::InvalidType,
                tool_call_index,
                tool_name,
                &format!("/{field_name}"),
                Some("array".to_string()),
                Some(json_type(other).to_string()),
                format!("{field_name} must be an array"),
                None,
            ))
        }
    };
    let mut out = Vec::with_capacity(array.len());
    for (index, value) in array.iter().enumerate() {
        let item = value.as_str().ok_or_else(|| {
            make_error(
                ToolContractErrorCode::InvalidType,
                tool_call_index,
                tool_name,
                &format!("/{field_name}/{index}"),
                Some("string".to_string()),
                Some(json_type(value).to_string()),
                format!("{field_name} entries must be strings"),
                None,
            )
        })?;
        let trimmed = item.trim();
        if trimmed.is_empty() {
            return Err(make_error(
                ToolContractErrorCode::InvalidValue,
                tool_call_index,
                tool_name,
                &format!("/{field_name}/{index}"),
                Some("non-empty string".to_string()),
                Some("empty string".to_string()),
                format!("{field_name} entries must be non-empty"),
                None,
            ));
        }
        out.push(trimmed.to_string());
    }
    Ok(out)
}

fn required_string_array(
    tool_call_index: usize,
    tool_name: &str,
    obj: &Map<String, Value>,
    field_name: &str,
) -> Result<Vec<String>, ContractError> {
    if !obj.contains_key(field_name) {
        return Err(make_error(
            ToolContractErrorCode::MissingRequiredField,
            tool_call_index,
            tool_name,
            &format!("/{field_name}"),
            Some("array".to_string()),
            None,
            format!("missing required field `{field_name}`"),
            None,
        ));
    }
    optional_string_array(tool_call_index, tool_name, obj, field_name)
}

fn optional_u64(
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
            ToolContractErrorCode::InvalidType,
            tool_call_index,
            tool_name,
            &format!("/{field}"),
            Some("integer|null".to_string()),
            Some(json_type(value).to_string()),
            format!("field `{field}` must be integer or null"),
            None,
        )
    })?;
    Ok(Some(number))
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
