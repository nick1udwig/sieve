use sieve_types::{Action, Capability, CommandKnowledge, CommandSummary, Resource, SinkCheck};

use crate::SummaryOutcome;

pub(crate) struct ArgvView<'a> {
    pub(crate) argv: &'a [String],
    pub(crate) offset: usize,
}

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

pub(crate) fn strip_sudo_with_offset(argv: &[String]) -> ArgvView<'_> {
    if basename(argv.first()).is_some_and(|cmd| cmd == "sudo") {
        if let Some(next) = command_after_wrapper_options(argv, 0, true, sudo_flag_value_words) {
            return ArgvView {
                argv: &argv[next..],
                offset: next,
            };
        }
    }
    ArgvView { argv, offset: 0 }
}

pub(crate) fn strip_env_wrapper_with_offset(argv: &[String]) -> ArgvView<'_> {
    if basename(argv.first()).is_some_and(|cmd| cmd == "env") {
        if let Some(next) = command_after_wrapper_options(argv, 0, true, env_flag_value_words) {
            return ArgvView {
                argv: &argv[next..],
                offset: next,
            };
        }
    }
    ArgvView { argv, offset: 0 }
}

fn command_after_wrapper_options(
    argv: &[String],
    start: usize,
    skip_assignments: bool,
    flag_value_words: fn(&str) -> Option<usize>,
) -> Option<usize> {
    let mut next = start + 1;
    while let Some(arg) = argv.get(next).map(String::as_str) {
        if skip_assignments && is_env_assignment(arg) {
            next += 1;
            continue;
        }
        let Some(value_words) = flag_value_words(arg) else {
            break;
        };
        if next + value_words >= argv.len() {
            return None;
        }
        next += 1 + value_words;
    }
    argv.get(next)
        .is_some_and(|arg| !arg.starts_with('-'))
        .then_some(next)
}

fn sudo_flag_value_words(token: &str) -> Option<usize> {
    const NO_VALUE_LONG: &[&str] = &[
        "askpass",
        "background",
        "non-interactive",
        "preserve-env",
        "reset-timestamp",
        "set-home",
        "stdin",
        "validate",
    ];
    const VALUE_LONG: &[&str] = &[
        "chdir",
        "close-from",
        "command-timeout",
        "group",
        "host",
        "login-class",
        "prompt",
        "role",
        "type",
        "user",
    ];
    if let Some(flag) = token.strip_prefix("--") {
        return long_flag_value_words(flag, NO_VALUE_LONG, VALUE_LONG);
    }
    short_flag_value_words(token, "AbEHknSv", "CDghprtTu")
}

fn env_flag_value_words(token: &str) -> Option<usize> {
    const NO_VALUE_LONG: &[&str] = &["debug", "ignore-environment", "null"];
    const VALUE_LONG: &[&str] = &["argv0", "chdir", "split-string", "unset"];
    if let Some(flag) = token.strip_prefix("--") {
        return long_flag_value_words(flag, NO_VALUE_LONG, VALUE_LONG);
    }
    short_flag_value_words(token, "0iv", "aCSu")
}

fn long_flag_value_words(flag: &str, no_value: &[&str], value: &[&str]) -> Option<usize> {
    if flag.is_empty() {
        return Some(0);
    }
    let name = flag.split_once('=').map_or(flag, |(name, _)| name);
    if no_value.contains(&name) {
        return Some(0);
    }
    value
        .contains(&name)
        .then_some(usize::from(!flag.contains('=')))
}

fn short_flag_value_words(token: &str, no_value: &str, value: &str) -> Option<usize> {
    if token == "--" {
        return Some(0);
    }
    if !token.starts_with('-') || token == "-" {
        return None;
    }
    let mut chars = token[1..].chars().peekable();
    while let Some(flag) = chars.next() {
        if no_value.contains(flag) {
            continue;
        }
        if value.contains(flag) {
            return Some(usize::from(chars.peek().is_none()));
        }
        return None;
    }
    Some(0)
}

fn is_env_assignment(arg: &str) -> bool {
    arg.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty())
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
