use super::{
    expect_object, json_type, optional_string, reject_unknown_fields, require_present,
    required_string,
};
use crate::{make_error, AutomationArgs, ContractError, ContractErrorParts, ToolContractErrorCode};
use serde_json::Value;
use sieve_types::{AutomationAction, AutomationRequest, AutomationSchedule, AutomationTarget};

pub(super) fn parse(
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
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::InvalidEnumVariant,
                    "/action",
                    "action is not a supported variant",
                )
                .expected("cron_list|cron_add|cron_remove|cron_pause|cron_resume")
                .found(args.action)
                .hint("use a supported automation action"),
            ))
        }
    };

    let target = match args.target {
        Some(target) => Some(match target.as_str() {
            "main" => AutomationTarget::Main,
            "isolated" => AutomationTarget::Isolated,
            _ => {
                return Err(make_error(
                    tool_call_index,
                    tool_name,
                    ContractErrorParts::new(
                        ToolContractErrorCode::InvalidEnumVariant,
                        "/target",
                        "target is not a supported variant",
                    )
                    .expected("main|isolated")
                    .found(target)
                    .hint("use `main` or `isolated`"),
                ))
            }
        }),
        None => None,
    };

    let schedule = match args.schedule {
        Some(raw) => Some(parse_schedule(tool_call_index, tool_name, &raw)?),
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

fn parse_schedule(
    tool_call_index: usize,
    tool_name: &str,
    value: &Value,
) -> Result<AutomationSchedule, ContractError> {
    let obj = match value {
        Value::Object(obj) => obj,
        other => {
            return Err(make_error(
                tool_call_index,
                tool_name,
                ContractErrorParts::new(
                    ToolContractErrorCode::InvalidType,
                    "/schedule",
                    "schedule must be an object",
                )
                .expected("object")
                .found(json_type(other))
                .hint("use a typed schedule object"),
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
            tool_call_index,
            tool_name,
            ContractErrorParts::new(
                ToolContractErrorCode::InvalidEnumVariant,
                "/schedule/kind",
                "schedule kind is not a supported variant",
            )
            .expected("after|at|every|cron")
            .found(kind)
            .hint("use `after`, `at`, `every`, or `cron`"),
        )),
    }
}
