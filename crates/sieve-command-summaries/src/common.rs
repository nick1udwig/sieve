use sieve_types::{Action, Capability, CommandKnowledge, CommandSummary, Resource};

use crate::SummaryOutcome;

pub(crate) fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    argv.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
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
    known_outcome(CommandSummary {
        required_capabilities: scopes
            .into_iter()
            .map(|scope| Capability {
                resource: Resource::Fs,
                action,
                scope,
            })
            .collect(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
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

pub(crate) fn strip_sudo(argv: &[String]) -> &[String] {
    if basename(argv.first()).is_some_and(|cmd| cmd == "sudo") && argv.len() > 1 {
        &argv[1..]
    } else {
        argv
    }
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
