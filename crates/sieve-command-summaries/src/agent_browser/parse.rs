use sieve_types::{Action, Capability, Resource};

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) index: usize,
    pub(super) value: String,
}

#[derive(Debug, Default)]
pub(super) struct ParsedGlobals {
    pub(super) remaining: Vec<Token>,
    pub(super) browser_capabilities: Vec<Capability>,
    pub(super) header_value_indices: Vec<usize>,
    pub(super) unsupported_flags: Vec<String>,
    pub(super) missing_value_flag: Option<String>,
}

const SUPPORTED_VALUE_FLAGS: &[&str] = &[
    "--session",
    "--profile",
    "--state",
    "--config",
    "--extension",
    "--download-path",
    "--action-policy",
];

pub(super) fn parse_globals(argv: &[String]) -> ParsedGlobals {
    let mut out = ParsedGlobals::default();
    let mut i = 1usize;

    while i < argv.len() {
        let arg = &argv[i];

        if matches!(arg.as_str(), "--json" | "--headed" | "--debug") {
            i += 1;
            continue;
        }

        if let Some((flag, parsed)) = parse_supported_value_flag(argv, i) {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            let consumed = parsed.consumed();
            add_capabilities_for_value_flag(&mut out.browser_capabilities, flag, parsed.value());
            i += consumed;
            continue;
        }

        if arg == "--headers" {
            if argv.get(i + 1).is_none() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.header_value_indices.push(i + 1);
            i += 2;
            continue;
        }
        if let Some((flag, value)) = crate::split_flag_value(arg) {
            if flag == "--headers" {
                if value.is_empty() {
                    out.missing_value_flag = Some(arg.clone());
                    break;
                }
                out.header_value_indices.push(i);
                i += 1;
                continue;
            }
            if is_unsupported_global_value_flag(flag) {
                if value.is_empty() {
                    out.missing_value_flag = Some(arg.clone());
                    break;
                }
                out.unsupported_flags.push(arg.clone());
                i += 1;
                continue;
            }
        }

        if matches!(
            arg.as_str(),
            "-p" | "--provider"
                | "--device"
                | "--proxy"
                | "--proxy-bypass"
                | "--cdp"
                | "--user-agent"
                | "--args"
                | "--color-scheme"
                | "--allowed-domains"
                | "--confirm-actions"
                | "--executable-path"
                | "--max-output"
                | "--session-name"
        ) {
            if argv.get(i + 1).is_none() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.unsupported_flags.push(arg.clone());
            i += 2;
            continue;
        }

        if matches!(
            arg.as_str(),
            "--auto-connect"
                | "--ignore-https-errors"
                | "--allow-file-access"
                | "--confirm-interactive"
                | "--content-boundaries"
                | "--native"
        ) {
            out.unsupported_flags.push(arg.clone());
            i += 1;
            continue;
        }

        out.remaining.push(Token {
            index: i,
            value: arg.clone(),
        });
        i += 1;
    }

    out
}

fn parse_supported_value_flag<'a>(
    argv: &'a [String],
    i: usize,
) -> Option<(&'static str, ParsedValueFlag<'a>)> {
    for flag in SUPPORTED_VALUE_FLAGS {
        if let Some(parsed) = parse_value_flag(argv, i, flag) {
            return Some((flag, parsed));
        }
    }
    None
}

fn add_capabilities_for_value_flag(capabilities: &mut Vec<Capability>, flag: &str, path: &str) {
    match flag {
        "--session" => {}
        "--profile" => {
            push_fs_capability(capabilities, Action::Read, path);
            push_fs_capability(capabilities, Action::Write, path);
        }
        "--download-path" => push_fs_capability(capabilities, Action::Write, path),
        "--state" | "--config" | "--extension" | "--action-policy" => {
            push_fs_capability(capabilities, Action::Read, path);
        }
        _ => unreachable!("unknown supported agent-browser global flag"),
    }
}

fn push_fs_capability(capabilities: &mut Vec<Capability>, action: Action, path: &str) {
    super::push_capability(capabilities, Resource::Fs, action, path.to_string());
}

enum ParsedValueFlag<'a> {
    Present { value: &'a str, consumed: usize },
    Missing,
}

impl ParsedValueFlag<'_> {
    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    fn consumed(&self) -> usize {
        match self {
            Self::Present { consumed, .. } => *consumed,
            Self::Missing => 1,
        }
    }

    fn value(&self) -> &str {
        match self {
            Self::Present { value, .. } => value,
            Self::Missing => "",
        }
    }
}

fn parse_value_flag<'a>(argv: &'a [String], i: usize, flag: &str) -> Option<ParsedValueFlag<'a>> {
    let arg = argv.get(i)?;
    if arg == flag {
        return Some(match argv.get(i + 1) {
            Some(value) => ParsedValueFlag::Present {
                value: value.as_str(),
                consumed: 2,
            },
            None => ParsedValueFlag::Missing,
        });
    }
    let (parsed_flag, value) = crate::split_flag_value(arg)?;
    (parsed_flag == flag).then_some(if value.is_empty() {
        ParsedValueFlag::Missing
    } else {
        ParsedValueFlag::Present { value, consumed: 1 }
    })
}

fn is_unsupported_global_value_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--provider"
            | "--device"
            | "--proxy"
            | "--proxy-bypass"
            | "--cdp"
            | "--user-agent"
            | "--args"
            | "--color-scheme"
            | "--allowed-domains"
            | "--confirm-actions"
            | "--executable-path"
            | "--max-output"
            | "--session-name"
    )
}
