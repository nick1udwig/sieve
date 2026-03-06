use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::SummaryOutcome;

const CODEX_API_CONNECT_SCOPE: &str = "https://api.openai.com/";

pub(super) fn summarize_codex_exec(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = super::strip_sudo(argv);
    if !is_codex_exec_command(inner) {
        return None;
    }

    let parsed = parse_exec_flags(inner, 2);
    if let Some(flag) = parsed.missing_value_flag {
        let reason = format!("codex exec flag missing value: {flag}");
        return Some(super::unknown_outcome(&reason));
    }
    if !parsed.unsupported_flags.is_empty() {
        return Some(super::unknown_with_flags(
            "unsupported codex exec flags",
            parsed.unsupported_flags,
        ));
    }
    if parsed.dangerously_bypass_approvals_and_sandbox {
        return Some(super::unknown_outcome(
            "codex exec dangerous bypass flag is unsupported",
        ));
    }

    let sandbox = parsed
        .sandbox
        .or_else(|| parsed.full_auto.then_some("workspace-write".to_string()));
    let Some(sandbox) = sandbox else {
        return Some(super::unknown_outcome("codex exec sandbox mode required"));
    };

    if sandbox == "danger-full-access" {
        return Some(super::unknown_outcome(
            "codex exec danger-full-access is unsupported",
        ));
    }

    let mut required_capabilities = vec![Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: CODEX_API_CONNECT_SCOPE.to_string(),
    }];

    match sandbox.as_str() {
        "read-only" => {
            if !parsed.ephemeral {
                return Some(super::unknown_outcome(
                    "codex exec read-only requires --ephemeral",
                ));
            }
            if parsed.output_last_message_path.is_some() {
                return Some(super::unknown_outcome(
                    "codex exec read-only forbids --output-last-message",
                ));
            }
        }
        "workspace-write" => {
            let mut write_scopes = Vec::new();
            write_scopes.push(parsed.cd_path.unwrap_or_else(|| ".".to_string()));
            write_scopes.extend(parsed.add_dirs);
            if let Some(path) = parsed.output_last_message_path {
                write_scopes.push(path);
            }

            for scope in dedupe_preserve_order(write_scopes) {
                required_capabilities.push(Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope,
                });
            }
        }
        _ => {
            return Some(super::unknown_outcome(
                "codex exec sandbox mode unsupported",
            ))
        }
    }

    Some(super::known_outcome(CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    }))
}

#[derive(Debug, Default)]
struct ParsedExecFlags {
    sandbox: Option<String>,
    full_auto: bool,
    ephemeral: bool,
    dangerously_bypass_approvals_and_sandbox: bool,
    cd_path: Option<String>,
    add_dirs: Vec<String>,
    output_last_message_path: Option<String>,
    unsupported_flags: Vec<String>,
    missing_value_flag: Option<String>,
}

fn parse_exec_flags(argv: &[String], start: usize) -> ParsedExecFlags {
    let mut out = ParsedExecFlags::default();
    let mut saw_end_of_flags = false;
    let mut i = start;

    while i < argv.len() {
        let arg = &argv[i];
        if saw_end_of_flags {
            i += 1;
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            i += 1;
            continue;
        }

        if arg == "--ephemeral" {
            out.ephemeral = true;
            i += 1;
            continue;
        }
        if arg == "--full-auto" {
            out.full_auto = true;
            i += 1;
            continue;
        }
        if arg == "--dangerously-bypass-approvals-and-sandbox" {
            out.dangerously_bypass_approvals_and_sandbox = true;
            i += 1;
            continue;
        }
        if matches!(
            arg.as_str(),
            "--search" | "--json" | "--skip-git-repo-check" | "--progress-cursor" | "--oss"
        ) {
            i += 1;
            continue;
        }

        if let Some(value) = parse_flag_value(arg, "-s", "--sandbox") {
            if value.is_empty() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.sandbox = Some(value.to_string());
            i += 1;
            continue;
        }
        if arg == "-s" || arg == "--sandbox" {
            let Some(value) = argv.get(i + 1) else {
                out.missing_value_flag = Some(arg.clone());
                break;
            };
            out.sandbox = Some(value.clone());
            i += 2;
            continue;
        }

        if let Some(value) = parse_flag_value(arg, "-C", "--cd") {
            if value.is_empty() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.cd_path = Some(value.to_string());
            i += 1;
            continue;
        }
        if arg == "-C" || arg == "--cd" {
            let Some(value) = argv.get(i + 1) else {
                out.missing_value_flag = Some(arg.clone());
                break;
            };
            out.cd_path = Some(value.clone());
            i += 2;
            continue;
        }

        if let Some(value) = parse_flag_value(arg, "-o", "--output-last-message") {
            if value.is_empty() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.output_last_message_path = Some(value.to_string());
            i += 1;
            continue;
        }
        if arg == "-o" || arg == "--output-last-message" {
            let Some(value) = argv.get(i + 1) else {
                out.missing_value_flag = Some(arg.clone());
                break;
            };
            out.output_last_message_path = Some(value.clone());
            i += 2;
            continue;
        }

        if let Some(value) = parse_flag_value(arg, "", "--add-dir") {
            if value.is_empty() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.add_dirs.push(value.to_string());
            i += 1;
            continue;
        }
        if arg == "--add-dir" {
            let Some(value) = argv.get(i + 1) else {
                out.missing_value_flag = Some(arg.clone());
                break;
            };
            out.add_dirs.push(value.clone());
            i += 2;
            continue;
        }

        if parse_ignored_value_flag(arg, &mut i, argv, &mut out) {
            continue;
        }

        out.unsupported_flags.push(arg.clone());
        i += 1;
    }

    out
}

fn parse_ignored_value_flag(
    arg: &str,
    i: &mut usize,
    argv: &[String],
    out: &mut ParsedExecFlags,
) -> bool {
    const SHORT_VALUE_FLAGS: &[char] = &['c', 'i', 'm', 'p'];
    const LONG_VALUE_FLAGS: &[&str] = &[
        "--config",
        "--enable",
        "--disable",
        "--image",
        "--model",
        "--local-provider",
        "--profile",
        "--output-schema",
        "--color",
    ];

    if let Some((flag, value)) = split_long_flag_value(arg) {
        if LONG_VALUE_FLAGS.contains(&flag) {
            if value.is_empty() {
                out.missing_value_flag = Some(arg.to_string());
            }
            *i += 1;
            return true;
        }
    }

    if let Some((flag, value)) = split_short_flag_value(arg) {
        if SHORT_VALUE_FLAGS.contains(&flag) {
            if value.is_empty() {
                out.missing_value_flag = Some(arg.to_string());
            }
            *i += 1;
            return true;
        }
    }

    if LONG_VALUE_FLAGS.contains(&arg) || matches!(arg, "-c" | "-i" | "-m" | "-p") {
        let Some(_value) = argv.get(*i + 1) else {
            out.missing_value_flag = Some(arg.to_string());
            *i += 1;
            return true;
        };
        *i += 2;
        return true;
    }

    false
}

fn parse_flag_value<'a>(arg: &'a str, short_flag: &str, long_flag: &str) -> Option<&'a str> {
    if !short_flag.is_empty() && arg.starts_with(short_flag) && arg.len() > short_flag.len() {
        return Some(&arg[short_flag.len()..]);
    }
    arg.strip_prefix(&format!("{long_flag}="))
}

fn split_long_flag_value(arg: &str) -> Option<(&str, &str)> {
    if !arg.starts_with("--") {
        return None;
    }
    let (flag, value) = arg.split_once('=')?;
    Some((flag, value))
}

fn split_short_flag_value(arg: &str) -> Option<(char, &str)> {
    if !arg.starts_with('-') || arg.starts_with("--") || arg.len() < 2 {
        return None;
    }
    let mut chars = arg.chars();
    if chars.next()? != '-' {
        return None;
    }
    let flag = chars.next()?;
    Some((flag, chars.as_str()))
}

fn dedupe_preserve_order(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

fn is_codex_exec_command(argv: &[String]) -> bool {
    if argv.len() < 2 {
        return false;
    }
    if super::basename(argv.first()).is_none_or(|cmd| cmd != "codex") {
        return false;
    }
    matches!(argv[1].as_str(), "exec" | "e")
}

#[cfg(test)]
mod tests;
