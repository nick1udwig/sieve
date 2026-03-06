use super::templates::is_subcommand_token;
use super::types::{CaseSource, PlannedCase};
use crate::fixture::{TOKEN_ARG, TOKEN_IN_FILE, TOKEN_KV, TOKEN_TMP_DIR, TOKEN_URL};
use std::collections::{BTreeSet, VecDeque};
use std::process::Command as StdCommand;

#[derive(Debug, Clone)]
struct HelpNode {
    command_path: Vec<String>,
    sample_args_from_parent_usage: Vec<String>,
    help_text: String,
}

#[derive(Debug, Clone)]
struct HelpSubcommandSpec {
    name: String,
    usage_tail: Vec<String>,
}

#[derive(Debug, Clone)]
struct HelpFlagSpec {
    flag: String,
    takes_value: bool,
    value_hint: Option<String>,
}

pub(super) fn discover_help_driven_cases(
    command: &str,
) -> Result<(Vec<PlannedCase>, Vec<Vec<String>>), String> {
    const HELP_DISCOVERY_MAX_DEPTH: usize = 4;
    const HELP_DISCOVERY_MAX_NODES: usize = 64;

    let mut queue = VecDeque::new();
    queue.push_back((Vec::<String>::new(), Vec::<String>::new()));

    let mut seen_paths = BTreeSet::new();
    let mut known_command_paths = Vec::new();
    let mut nodes = Vec::new();

    while let Some((command_path, sample_args_from_parent_usage)) = queue.pop_front() {
        if command_path.len() > HELP_DISCOVERY_MAX_DEPTH {
            continue;
        }
        let key = command_path.join("\u{1f}");
        if !seen_paths.insert(key) {
            continue;
        }
        if seen_paths.len() > HELP_DISCOVERY_MAX_NODES {
            break;
        }

        let help_text = match read_help_text(command, &command_path) {
            Ok(help_text) => help_text,
            Err(err) => {
                if command_path.is_empty() {
                    return Err(err);
                }
                continue;
            }
        };
        let subcommands = parse_subcommands_from_help(&help_text);
        known_command_paths.push(command_path.clone());
        nodes.push(HelpNode {
            command_path: command_path.clone(),
            sample_args_from_parent_usage,
            help_text,
        });

        if command_path.len() >= HELP_DISCOVERY_MAX_DEPTH {
            continue;
        }
        for subcommand in subcommands {
            if subcommand.name.eq_ignore_ascii_case("help") {
                continue;
            }
            let mut child_path = command_path.clone();
            child_path.push(subcommand.name);
            queue.push_back((
                child_path,
                sample_args_from_usage_tail(&subcommand.usage_tail),
            ));
            if seen_paths.len() + queue.len() > HELP_DISCOVERY_MAX_NODES {
                break;
            }
        }
    }

    let mut planned_cases = Vec::new();
    for node in nodes {
        let mut argv_prefix = vec![command.to_string()];
        argv_prefix.extend(node.command_path.clone());

        if !node.command_path.is_empty() {
            let mut help_case = argv_prefix.clone();
            help_case.push("--help".to_string());
            planned_cases.push(PlannedCase {
                command_path: node.command_path.clone(),
                argv_template: help_case,
                source: CaseSource::HelpDiscovery,
            });
        }

        if !node.sample_args_from_parent_usage.is_empty() {
            let mut exercise_case = argv_prefix.clone();
            exercise_case.extend(node.sample_args_from_parent_usage.clone());
            planned_cases.push(PlannedCase {
                command_path: node.command_path.clone(),
                argv_template: exercise_case,
                source: CaseSource::HelpDiscovery,
            });
        }

        planned_cases.extend(generate_flag_exercise_cases(&node, command));
    }

    Ok((planned_cases, known_command_paths))
}

fn read_help_text(command: &str, command_path: &[String]) -> Result<String, String> {
    let mut help_command = StdCommand::new(command);
    help_command.args(command_path);
    help_command.arg("--help");
    let output = help_command.output().map_err(|err| {
        format!(
            "`{} --help` failed: {err}",
            command_with_path(command, command_path)
        )
    })?;

    let mut help_text = String::new();
    help_text.push_str(String::from_utf8_lossy(&output.stdout).as_ref());
    if !output.stderr.is_empty() {
        if !help_text.is_empty() && !help_text.ends_with('\n') {
            help_text.push('\n');
        }
        help_text.push_str(String::from_utf8_lossy(&output.stderr).as_ref());
    }
    if help_text.trim().is_empty() {
        return Err(format!(
            "`{} --help` produced empty output",
            command_with_path(command, command_path)
        ));
    }
    Ok(help_text)
}

fn command_with_path(command: &str, command_path: &[String]) -> String {
    let mut joined = command.to_string();
    for segment in command_path {
        joined.push(' ');
        joined.push_str(segment);
    }
    joined
}

fn parse_subcommands_from_help(help_text: &str) -> Vec<HelpSubcommandSpec> {
    let mut in_commands_section = false;
    let mut found_any = false;
    let mut seen = BTreeSet::new();
    let mut subcommands = Vec::new();

    for line in help_text.lines() {
        let trimmed = line.trim();
        if !in_commands_section {
            if is_commands_header(trimmed) {
                in_commands_section = true;
            }
            continue;
        }

        if trimmed.is_empty() {
            if found_any {
                break;
            }
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            if found_any {
                break;
            }
            continue;
        }

        let command_spec = split_help_spec_column(trimmed);
        let mut parts = command_spec.split_whitespace();
        let Some(raw_token) = parts.next() else {
            continue;
        };
        let token = raw_token.trim_end_matches([',', ':']);
        if !is_subcommand_token(token) {
            if found_any {
                break;
            }
            continue;
        }

        let token = token.to_string();
        found_any = true;
        if seen.insert(token.clone()) {
            let usage_tail = parts.map(ToString::to_string).collect();
            subcommands.push(HelpSubcommandSpec {
                name: token,
                usage_tail,
            });
        }
    }

    subcommands
}

fn is_commands_header(line: &str) -> bool {
    matches!(
        line.to_ascii_lowercase().as_str(),
        "commands:" | "subcommands:" | "available commands:" | "available subcommands:"
    )
}

fn split_help_spec_column(line: &str) -> &str {
    let bytes = line.as_bytes();
    for idx in 0..bytes.len() {
        if bytes[idx] == b'\t' {
            return line[..idx].trim_end();
        }
        if idx + 1 < bytes.len() && bytes[idx] == b' ' && bytes[idx + 1] == b' ' {
            return line[..idx].trim_end();
        }
    }
    line.trim_end()
}

fn sample_args_from_usage_tail(usage_tail: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for token in usage_tail {
        let normalized = normalize_usage_token(token);
        if normalized.is_empty() {
            continue;
        }

        if normalized.starts_with('<') && normalized.ends_with('>') {
            let placeholder = &normalized[1..normalized.len() - 1];
            args.push(sample_value_for_placeholder(placeholder));
            continue;
        }
        if normalized.starts_with("--") || normalized.starts_with('-') {
            args.push(normalized);
            continue;
        }

        if normalized.eq_ignore_ascii_case("query") {
            args.push(TOKEN_ARG.to_string());
            continue;
        }
    }
    args
}

fn normalize_usage_token(token: &str) -> String {
    token
        .trim_matches(|ch| matches!(ch, '[' | ']' | '(' | ')' | ',' | ':'))
        .to_string()
}

fn sample_value_for_placeholder(placeholder: &str) -> String {
    let lowered = placeholder.to_ascii_lowercase();
    if lowered.contains("query") || lowered.contains("key") || lowered.contains("value") {
        TOKEN_ARG.to_string()
    } else if lowered.contains("file") || lowered.contains("path") || lowered.contains("config") {
        TOKEN_IN_FILE.to_string()
    } else if lowered.contains("url") || lowered.contains("uri") || lowered.contains("endpoint") {
        TOKEN_URL.to_string()
    } else {
        TOKEN_ARG.to_string()
    }
}

fn generate_flag_exercise_cases(node: &HelpNode, command: &str) -> Vec<PlannedCase> {
    const HELP_FLAG_CASE_LIMIT_PER_COMMAND: usize = 2;

    let flags = parse_flags_from_help(&node.help_text);
    if flags.is_empty() {
        return Vec::new();
    }

    let mut chosen_cases = Vec::new();

    if let Some(query_flag) = flags.iter().find(|flag| is_query_flag(&flag.flag)) {
        let mut args = vec![query_flag.flag.clone()];
        if query_flag.takes_value {
            args.push(sample_value_for_flag(query_flag));
        }
        chosen_cases.push(args);
    }

    for flag in flags {
        if chosen_cases.len() >= HELP_FLAG_CASE_LIMIT_PER_COMMAND {
            break;
        }
        if is_help_flag(&flag.flag) || is_query_flag(&flag.flag) {
            continue;
        }

        let mut args = vec![flag.flag.clone()];
        if flag.takes_value {
            args.push(sample_value_for_flag(&flag));
        }
        chosen_cases.push(args);
    }

    let mut out = Vec::new();
    for args in chosen_cases {
        let mut argv_template = vec![command.to_string()];
        argv_template.extend(node.command_path.clone());
        argv_template.extend(args);
        out.push(PlannedCase {
            command_path: node.command_path.clone(),
            argv_template,
            source: CaseSource::HelpDiscovery,
        });
    }
    out
}

fn parse_flags_from_help(help_text: &str) -> Vec<HelpFlagSpec> {
    let mut seen = BTreeSet::new();
    let mut flags = Vec::new();

    for line in help_text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('-') {
            continue;
        }

        let spec = split_help_spec_column(trimmed);
        let aliases: Vec<&str> = spec.split(',').map(str::trim).collect();
        let mut parsed = Vec::new();

        for alias in aliases {
            let mut parts = alias.split_whitespace();
            let Some(flag_token_raw) = parts.next() else {
                continue;
            };
            let flag_token = flag_token_raw.trim_end_matches([',', ':']);
            if !flag_token.starts_with('-') || flag_token == "-" || flag_token == "--" {
                continue;
            }

            let value_hint = parts.next().map(|value| value.to_string());
            parsed.push((flag_token.to_string(), value_hint));
        }

        if parsed.is_empty() {
            continue;
        }

        let mut chosen = parsed[0].clone();
        for entry in &parsed {
            if entry.0.starts_with("--") {
                chosen = entry.clone();
            }
        }

        let takes_value = parsed.iter().any(|entry| entry.1.is_some());
        let value_hint = if chosen.1.is_some() {
            chosen.1.clone()
        } else {
            parsed.iter().find_map(|entry| entry.1.clone())
        };

        if seen.insert(chosen.0.clone()) {
            flags.push(HelpFlagSpec {
                flag: chosen.0,
                takes_value,
                value_hint,
            });
        }
    }

    flags
}

fn is_help_flag(flag: &str) -> bool {
    matches!(flag, "-h" | "--help")
}

fn is_query_flag(flag: &str) -> bool {
    matches!(flag, "-q" | "--q" | "--query")
}

fn sample_value_for_flag(flag: &HelpFlagSpec) -> String {
    let lowered = flag.flag.to_ascii_lowercase();
    if lowered.contains("query") || lowered == "--q" || lowered == "-q" {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("count") || lowered.contains("offset") || lowered.contains("retries") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("country") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("search-lang") || lowered.contains("lang") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("ui-lang") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("safesearch") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("freshness") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("timeout") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("cache-ttl") || lowered.contains("ttl") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("output") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("api-key-file") || lowered == "-config" {
        return TOKEN_IN_FILE.to_string();
    }
    if lowered.contains("config") {
        return TOKEN_IN_FILE.to_string();
    }
    if lowered.contains("cache-dir") || lowered.contains("temp-dir") {
        return TOKEN_TMP_DIR.to_string();
    }
    if lowered.contains("param") {
        return TOKEN_KV.to_string();
    }
    if lowered.contains("goggle") {
        return TOKEN_URL.to_string();
    }
    if lowered.contains("api-key") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("version") {
        return TOKEN_ARG.to_string();
    }
    if let Some(hint) = flag.value_hint.as_deref() {
        let hint_lowered = hint.to_ascii_lowercase();
        if hint_lowered.contains("int") {
            return TOKEN_ARG.to_string();
        }
        if hint_lowered.contains("duration") {
            return TOKEN_ARG.to_string();
        }
        if hint_lowered.contains("value") {
            return TOKEN_ARG.to_string();
        }
    }
    TOKEN_ARG.to_string()
}
