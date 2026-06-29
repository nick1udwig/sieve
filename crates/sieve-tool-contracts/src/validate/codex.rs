use super::{
    expect_object, optional_string_array, optional_trimmed_string, optional_u64,
    reject_unknown_fields, required_string, required_string_array,
};
use crate::{
    make_error, CodexExecArgs, CodexSessionArgs, ContractError, ContractErrorParts,
    ToolContractErrorCode,
};
use serde_json::Value;
use sieve_types::{CodexExecRequest, CodexSandboxMode, CodexSessionRequest};

pub(super) fn parse_exec(
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
        cwd: optional_trimmed_string(tool_call_index, tool_name, obj, "cwd")?,
        writable_roots: optional_string_array(tool_call_index, tool_name, obj, "writable_roots")?,
        timeout_ms: optional_u64(tool_call_index, tool_name, obj, "timeout_ms")?,
    };
    Ok(CodexExecRequest {
        command: non_empty_command(tool_call_index, tool_name, &args.command)?,
        sandbox: parse_sandbox(tool_call_index, tool_name, &args.sandbox, "/sandbox")?,
        cwd: args.cwd,
        writable_roots: args.writable_roots,
        timeout_ms: args.timeout_ms,
    })
}

pub(super) fn parse_session(
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
        session_id: optional_trimmed_string(tool_call_index, tool_name, obj, "session_id")?,
        instruction: required_string(tool_call_index, tool_name, obj, "instruction")?,
        sandbox: required_string(tool_call_index, tool_name, obj, "sandbox")?,
        cwd: optional_trimmed_string(tool_call_index, tool_name, obj, "cwd")?,
        writable_roots: optional_string_array(tool_call_index, tool_name, obj, "writable_roots")?,
        local_images: optional_string_array(tool_call_index, tool_name, obj, "local_images")?,
    };
    Ok(CodexSessionRequest {
        session_id: args.session_id,
        instruction: non_empty_instruction(tool_call_index, tool_name, &args.instruction)?,
        sandbox: parse_sandbox(tool_call_index, tool_name, &args.sandbox, "/sandbox")?,
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
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/instruction",
                "instruction must be non-empty",
            )
            .expected("non-empty string")
            .found("empty string")
            .hint("pass task instructions for Codex"),
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
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidValue,
                "/command",
                "command must include at least one argv entry",
            )
            .expected("non-empty array")
            .found("empty array")
            .hint("pass command as argv array, for example [\"git\",\"status\"]"),
        ));
    }
    Ok(command.to_vec())
}

fn parse_sandbox(
    tool_call_index: usize,
    tool_name: &str,
    raw: &str,
    argument_path: &str,
) -> Result<CodexSandboxMode, ContractError> {
    match raw {
        "read_only" => Ok(CodexSandboxMode::ReadOnly),
        "workspace_write" => Ok(CodexSandboxMode::WorkspaceWrite),
        _ => Err(make_error(
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidEnumVariant,
                argument_path,
                "sandbox is not a supported variant",
            )
            .expected("read_only|workspace_write")
            .found(raw)
            .hint("use `read_only` or `workspace_write`"),
        )),
    }
}
