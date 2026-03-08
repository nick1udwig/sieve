use super::{parse_at_timestamp_ms, parse_duration_ms, CronJobSchedule, CronSessionTarget};
use sieve_types::{AutomationAction, AutomationRequest, AutomationScheduleKind, AutomationTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AutomationCommand {
    HeartbeatNow,
    CronList,
    CronAdd {
        target: CronSessionTarget,
        schedule: CronJobSchedule,
        prompt: String,
    },
    CronRemove {
        job_id: String,
    },
    CronPause {
        job_id: String,
    },
    CronResume {
        job_id: String,
    },
}

pub(crate) fn parse_automation_command(
    input: &str,
    now_ms: u64,
) -> Result<Option<AutomationCommand>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !trimmed.starts_with('/') {
        return Ok(None);
    }

    if trimmed == "/heartbeat now" {
        return Ok(Some(AutomationCommand::HeartbeatNow));
    }
    if trimmed == "/cron list" {
        return Ok(Some(AutomationCommand::CronList));
    }

    if let Some(rest) = trimmed.strip_prefix("/cron rm ") {
        return Ok(Some(AutomationCommand::CronRemove {
            job_id: rest.trim().to_string(),
        }));
    }
    if let Some(rest) = trimmed.strip_prefix("/cron remove ") {
        return Ok(Some(AutomationCommand::CronRemove {
            job_id: rest.trim().to_string(),
        }));
    }
    if let Some(rest) = trimmed.strip_prefix("/cron pause ") {
        return Ok(Some(AutomationCommand::CronPause {
            job_id: rest.trim().to_string(),
        }));
    }
    if let Some(rest) = trimmed.strip_prefix("/cron resume ") {
        return Ok(Some(AutomationCommand::CronResume {
            job_id: rest.trim().to_string(),
        }));
    }

    let Some(rest) = trimmed.strip_prefix("/cron add ") else {
        return Ok(None);
    };
    let Some((lhs, prompt)) = rest.split_once(" -- ") else {
        return Err(
            "cron add syntax: /cron add <main|isolated> <every|at|cron> <schedule> -- <prompt>"
                .to_string(),
        );
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("cron add prompt cannot be empty".to_string());
    }

    let parts = lhs
        .split_whitespace()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 3 {
        return Err(
            "cron add syntax: /cron add <main|isolated> <every|at|cron> <schedule> -- <prompt>"
                .to_string(),
        );
    }

    let target = match parts[0] {
        "main" => CronSessionTarget::Main,
        "isolated" => CronSessionTarget::Isolated,
        other => {
            return Err(format!(
                "unsupported cron target `{other}`; expected `main` or `isolated`"
            ))
        }
    };
    let schedule = match parts[1] {
        "every" => {
            if parts.len() != 3 {
                return Err("`every` schedule expects exactly one duration argument".to_string());
            }
            CronJobSchedule::Every {
                every_ms: parse_duration_ms(parts[2])?,
                anchor_ms: now_ms,
            }
        }
        "at" => {
            if parts.len() != 3 {
                return Err(
                    "`at` schedule expects exactly one RFC3339 or unix-ms timestamp".to_string(),
                );
            }
            CronJobSchedule::At {
                at_ms: parse_at_timestamp_ms(parts[2])?,
            }
        }
        "cron" => {
            let expr = parts[2..].join(" ");
            if expr.trim().is_empty() {
                return Err("`cron` schedule requires an expression".to_string());
            }
            CronJobSchedule::Cron { expr }
        }
        other => {
            return Err(format!(
                "unsupported schedule kind `{other}`; expected `every`, `at`, or `cron`"
            ))
        }
    };

    Ok(Some(AutomationCommand::CronAdd {
        target,
        schedule,
        prompt: prompt.to_string(),
    }))
}

pub(crate) fn automation_command_from_request(
    request: AutomationRequest,
    now_ms: u64,
) -> Result<AutomationCommand, String> {
    match request.action {
        AutomationAction::CronList => Ok(AutomationCommand::CronList),
        AutomationAction::CronAdd => {
            let target = match request.target {
                Some(AutomationTarget::Main) => CronSessionTarget::Main,
                Some(AutomationTarget::Isolated) => CronSessionTarget::Isolated,
                None => return Err("cron_add requires target".to_string()),
            };
            let schedule_kind = request
                .schedule_kind
                .ok_or_else(|| "cron_add requires schedule_kind".to_string())?;
            let schedule_raw = request
                .schedule
                .ok_or_else(|| "cron_add requires schedule".to_string())?;
            let schedule = match schedule_kind {
                AutomationScheduleKind::Every => CronJobSchedule::Every {
                    every_ms: parse_duration_ms(&schedule_raw)?,
                    anchor_ms: now_ms,
                },
                AutomationScheduleKind::At => CronJobSchedule::At {
                    at_ms: parse_at_timestamp_ms(&schedule_raw)?,
                },
                AutomationScheduleKind::Cron => CronJobSchedule::Cron { expr: schedule_raw },
            };
            let prompt = request
                .prompt
                .ok_or_else(|| "cron_add requires prompt".to_string())?;
            Ok(AutomationCommand::CronAdd {
                target,
                schedule,
                prompt,
            })
        }
        AutomationAction::CronRemove => Ok(AutomationCommand::CronRemove {
            job_id: request
                .job_id
                .ok_or_else(|| "cron_remove requires job_id".to_string())?,
        }),
        AutomationAction::CronPause => Ok(AutomationCommand::CronPause {
            job_id: request
                .job_id
                .ok_or_else(|| "cron_pause requires job_id".to_string())?,
        }),
        AutomationAction::CronResume => Ok(AutomationCommand::CronResume {
            job_id: request
                .job_id
                .ok_or_else(|| "cron_resume requires job_id".to_string())?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_non_commands() {
        assert_eq!(
            parse_automation_command("review logs", 1_000).expect("parse"),
            None
        );
    }

    #[test]
    fn parses_heartbeat_now() {
        assert_eq!(
            parse_automation_command("/heartbeat now", 1_000).expect("parse"),
            Some(AutomationCommand::HeartbeatNow)
        );
    }

    #[test]
    fn parses_cron_add_every() {
        let command = parse_automation_command(
            "/cron add main every 15m -- remind me to check deploys",
            5_000,
        )
        .expect("parse")
        .expect("command");
        assert_eq!(
            command,
            AutomationCommand::CronAdd {
                target: CronSessionTarget::Main,
                schedule: CronJobSchedule::Every {
                    every_ms: 900_000,
                    anchor_ms: 5_000,
                },
                prompt: "remind me to check deploys".to_string(),
            }
        );
    }

    #[test]
    fn parses_cron_add_cron_expression() {
        let command = parse_automation_command(
            "/cron add isolated cron 0 9 * * 1-5 -- send status summary",
            5_000,
        )
        .expect("parse")
        .expect("command");
        assert_eq!(
            command,
            AutomationCommand::CronAdd {
                target: CronSessionTarget::Isolated,
                schedule: CronJobSchedule::Cron {
                    expr: "0 9 * * 1-5".to_string(),
                },
                prompt: "send status summary".to_string(),
            }
        );
    }

    #[test]
    fn parses_cron_mutation_commands() {
        assert_eq!(
            parse_automation_command("/cron list", 1_000).expect("list"),
            Some(AutomationCommand::CronList)
        );
        assert_eq!(
            parse_automation_command("/cron rm cron-1", 1_000).expect("rm"),
            Some(AutomationCommand::CronRemove {
                job_id: "cron-1".to_string(),
            })
        );
        assert_eq!(
            parse_automation_command("/cron pause cron-2", 1_000).expect("pause"),
            Some(AutomationCommand::CronPause {
                job_id: "cron-2".to_string(),
            })
        );
        assert_eq!(
            parse_automation_command("/cron resume cron-3", 1_000).expect("resume"),
            Some(AutomationCommand::CronResume {
                job_id: "cron-3".to_string(),
            })
        );
    }
}
