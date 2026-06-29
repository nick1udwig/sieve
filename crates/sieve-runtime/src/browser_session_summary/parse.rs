use sieve_types::{Action, Capability, Resource};
use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct ParsedAgentBrowser {
    pub(crate) remaining: Vec<Token>,
    pub(crate) session_name: Option<String>,
    pub(crate) global_capabilities: Vec<Capability>,
    pub(crate) header_value_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) index: usize,
    pub(crate) value: String,
}

pub(crate) fn parse_agent_browser(argv: &[String]) -> Option<ParsedAgentBrowser> {
    let inner = strip_command_wrappers(argv);
    if basename(inner.argv.first()) != Some("agent-browser") {
        return None;
    }

    let mut remaining = Vec::new();
    let mut session_name = None;
    let mut global_capabilities = Vec::new();
    let mut header_value_indices = Vec::new();
    let mut i = 1usize;
    while i < inner.argv.len() {
        let arg = &inner.argv[i];
        if matches!(arg.as_str(), "--json" | "--headed" | "--debug") {
            i += 1;
            continue;
        }
        if let Some(parsed) = parse_value_flag(inner.argv, i, "--session") {
            session_name = Some(parsed.value.to_string());
            i += parsed.consumed;
            continue;
        }
        if let Some((parsed, actions)) = parse_global_capability_value_flag(inner.argv, i) {
            for (resource, action) in actions {
                super::push_capability(
                    &mut global_capabilities,
                    *resource,
                    *action,
                    parsed.value.to_string(),
                );
            }
            i += parsed.consumed;
            continue;
        }
        if let Some(parsed) = parse_value_flag(inner.argv, i, "--headers") {
            header_value_indices.push(parsed.value_index + inner.offset);
            i += parsed.consumed;
            continue;
        }
        if should_skip_value_flag(inner.argv, i) {
            i += if arg.contains('=') { 1 } else { 2 };
            continue;
        }
        if should_skip_bool_flag(arg) {
            i += 1;
            continue;
        }
        remaining.push(Token {
            index: i + inner.offset,
            value: arg.clone(),
        });
        i += 1;
    }

    Some(ParsedAgentBrowser {
        remaining,
        session_name,
        global_capabilities,
        header_value_indices,
    })
}

struct ArgvView<'a> {
    argv: &'a [String],
    offset: usize,
}

fn strip_command_wrappers(mut argv: &[String]) -> ArgvView<'_> {
    let mut offset = 0usize;
    loop {
        let sudo = strip_sudo_wrapper(argv);
        if sudo.offset > 0 {
            offset += sudo.offset;
            argv = sudo.argv;
            continue;
        }
        let env = strip_env_wrapper(argv);
        if env.offset > 0 {
            offset += env.offset;
            argv = env.argv;
            continue;
        }
        return ArgvView { argv, offset };
    }
}

fn strip_sudo_wrapper(argv: &[String]) -> ArgvView<'_> {
    if basename(argv.first()) == Some("sudo") {
        if let Some(next) = command_after_wrapper_options(argv, 0, true, sudo_flag_value_words) {
            return ArgvView {
                argv: &argv[next..],
                offset: next,
            };
        }
    }
    ArgvView { argv, offset: 0 }
}

fn strip_env_wrapper(argv: &[String]) -> ArgvView<'_> {
    if basename(argv.first()) == Some("env") {
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

fn basename(s: Option<&String>) -> Option<&str> {
    let s = s?;
    Path::new(s).file_name().and_then(|part| part.to_str())
}

struct ParsedValueFlag<'a> {
    value: &'a str,
    consumed: usize,
    value_index: usize,
}

struct GlobalCapabilityValueFlag {
    name: &'static str,
    actions: &'static [(Resource, Action)],
}

const GLOBAL_CAPABILITY_VALUE_FLAGS: &[GlobalCapabilityValueFlag] = &[
    GlobalCapabilityValueFlag {
        name: "--profile",
        actions: &[(Resource::Fs, Action::Read), (Resource::Fs, Action::Write)],
    },
    GlobalCapabilityValueFlag {
        name: "--state",
        actions: &[(Resource::Fs, Action::Read)],
    },
    GlobalCapabilityValueFlag {
        name: "--config",
        actions: &[(Resource::Fs, Action::Read)],
    },
    GlobalCapabilityValueFlag {
        name: "--extension",
        actions: &[(Resource::Fs, Action::Read)],
    },
    GlobalCapabilityValueFlag {
        name: "--download-path",
        actions: &[(Resource::Fs, Action::Write)],
    },
    GlobalCapabilityValueFlag {
        name: "--action-policy",
        actions: &[(Resource::Fs, Action::Read)],
    },
];

fn parse_global_capability_value_flag<'a>(
    argv: &'a [String],
    i: usize,
) -> Option<(ParsedValueFlag<'a>, &'static [(Resource, Action)])> {
    GLOBAL_CAPABILITY_VALUE_FLAGS
        .iter()
        .find_map(|flag| parse_value_flag(argv, i, flag.name).map(|parsed| (parsed, flag.actions)))
}

fn parse_value_flag<'a>(argv: &'a [String], i: usize, flag: &str) -> Option<ParsedValueFlag<'a>> {
    let arg = argv.get(i)?;
    if arg == flag {
        return argv.get(i + 1).map(|value| ParsedValueFlag {
            value: value.as_str(),
            consumed: 2,
            value_index: i + 1,
        });
    }
    let (parsed_flag, value) = arg.split_once('=')?;
    (parsed_flag == flag && !value.is_empty()).then_some(ParsedValueFlag {
        value,
        consumed: 1,
        value_index: i,
    })
}

fn should_skip_value_flag(argv: &[String], i: usize) -> bool {
    const VALUE_FLAGS: &[&str] = &[
        "--provider",
        "--device",
        "--proxy",
        "--proxy-bypass",
        "--cdp",
        "--user-agent",
        "--args",
        "--color-scheme",
        "--allowed-domains",
        "--confirm-actions",
        "--executable-path",
        "--max-output",
        "--session-name",
        "-p",
    ];
    let Some(arg) = argv.get(i) else {
        return false;
    };
    if VALUE_FLAGS.contains(&arg.as_str()) {
        return true;
    }
    arg.split_once('=')
        .is_some_and(|(flag, value)| VALUE_FLAGS.contains(&flag) && !value.is_empty())
}

fn should_skip_bool_flag(arg: &str) -> bool {
    matches!(
        arg,
        "--auto-connect"
            | "--ignore-https-errors"
            | "--allow-file-access"
            | "--confirm-interactive"
            | "--content-boundaries"
            | "--native"
    )
}
