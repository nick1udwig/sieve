use sieve_types::{Action, Capability, CommandKnowledge, CommandSummary, Resource, SinkCheck};

use crate::SummaryOutcome;

pub(crate) fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    for (i, arg) in argv.iter().enumerate() {
        if arg == "--" {
            return None;
        }
        if let Some((name, value)) = split_flag_value(arg) {
            if name == flag {
                return Some(value.to_string());
            }
        }
        if arg == flag {
            return argv.get(i + 1).cloned();
        }
    }
    None
}

pub(crate) fn split_flag_value(flag: &str) -> Option<(&str, &str)> {
    let eq = flag.find('=')?;
    Some((&flag[..eq], &flag[eq + 1..]))
}

pub(crate) fn collect_positionals_with_no_value_flags(
    argv: &[String],
    allowed_short_flags: &[char],
    allowed_long_flags: &[&str],
    allowed_long_prefixes: &[&str],
) -> (Vec<String>, Vec<String>) {
    let mut positionals = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;

    for arg in argv.iter().skip(1) {
        if saw_end_of_flags {
            positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            positionals.push(arg.clone());
            continue;
        }

        if arg.starts_with("--") {
            if allowed_long_flags.contains(&arg.as_str())
                || allowed_long_prefixes
                    .iter()
                    .any(|prefix| arg.starts_with(prefix))
            {
                continue;
            }
            unsupported_flags.push(arg.clone());
            continue;
        }

        if is_short_flag_cluster(arg, allowed_short_flags) {
            continue;
        }
        unsupported_flags.push(arg.clone());
    }

    (positionals, unsupported_flags)
}

pub(crate) fn known_fs_outcome(scopes: Vec<String>, action: Action) -> SummaryOutcome {
    known_capabilities_outcome(
        scopes
            .into_iter()
            .map(|scope| Capability {
                resource: Resource::Fs,
                action,
                scope,
            })
            .collect(),
    )
}

pub(crate) fn known_capabilities_outcome(required_capabilities: Vec<Capability>) -> SummaryOutcome {
    known_summary_outcome(required_capabilities, Vec::new())
}

pub(crate) fn known_summary_outcome(
    required_capabilities: Vec<Capability>,
    sink_checks: Vec<SinkCheck>,
) -> SummaryOutcome {
    known_outcome(CommandSummary {
        required_capabilities,
        sink_checks,
        unsupported_flags: Vec::new(),
    })
}

pub(crate) fn known_empty_outcome() -> SummaryOutcome {
    known_capabilities_outcome(Vec::new())
}

pub(crate) fn is_short_flag_cluster(arg: &str, allowed_flags: &[char]) -> bool {
    arg.len() > 1
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg[1..].chars().all(|ch| allowed_flags.contains(&ch))
}

pub(crate) fn is_named_command(argv: &[String], command: &str) -> bool {
    basename(argv.first()).is_some_and(|cmd| cmd == command)
}

pub(crate) fn is_curl_command(argv: &[String]) -> bool {
    is_named_command(argv, "curl")
}

pub(crate) fn basename(s: Option<&String>) -> Option<&str> {
    let s = s?;
    std::path::Path::new(s)
        .file_name()
        .and_then(|part| part.to_str())
}

pub(crate) fn known_outcome(summary: CommandSummary) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Known,
        summary: Some(summary),
        reason: None,
    }
}

pub(crate) fn unknown_outcome(reason: &str) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Unknown,
        summary: None,
        reason: Some(reason.to_string()),
    }
}

pub(crate) fn rebase_argv_value_refs(mut outcome: SummaryOutcome, offset: usize) -> SummaryOutcome {
    if offset == 0 {
        return outcome;
    }
    let Some(summary) = &mut outcome.summary else {
        return outcome;
    };
    for sink_check in &mut summary.sink_checks {
        for value_ref in &mut sink_check.value_refs {
            if let Some(index) = value_ref
                .0
                .strip_prefix("argv:")
                .and_then(|raw| raw.parse::<usize>().ok())
            {
                value_ref.0 = format!("argv:{}", index + offset);
            }
        }
    }
    outcome
}

pub(crate) fn unknown_with_flags(reason: &str, unsupported_flags: Vec<String>) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Unknown,
        summary: Some(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags,
        }),
        reason: Some(reason.to_string()),
    }
}
