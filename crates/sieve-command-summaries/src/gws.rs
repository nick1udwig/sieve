use sieve_types::{Action, Capability, CommandSummary, Resource, SinkCheck, SinkKey, ValueRef};

use crate::{is_named_command, known_outcome, strip_sudo, unknown_outcome, unknown_with_flags};

const GWS_API_ORIGIN: &str = "https://www.googleapis.com/";
const GWS_UPLOAD_ORIGIN: &str = "https://www.googleapis.com/upload/";
const GWS_MODELARMOR_ORIGIN: &str = "https://modelarmor.googleapis.com/";

pub(super) fn summarize_gws(argv: &[String]) -> Option<crate::SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "gws") {
        return None;
    }

    if inner.len() < 2 {
        return Some(known_noop_outcome());
    }

    let root = inner[1].as_str();
    Some(match root {
        "-h" | "--help" | "help" | "version" | "--version" | "-V" => known_noop_outcome(),
        "schema" => summarize_schema(inner),
        "auth" => unknown_outcome("gws auth commands are unsupported"),
        "mcp" => unknown_outcome("gws mcp is unsupported"),
        "workflow" => unknown_outcome("gws workflow helpers are unsupported"),
        "modelarmor" => unknown_outcome("gws modelarmor helpers are unsupported"),
        other if other.starts_with('+') => unknown_outcome("gws top-level helpers are unsupported"),
        _ => summarize_service(inner),
    })
}

#[derive(Debug, Clone)]
struct FlagArg {
    argument_name: String,
    value_index: usize,
}

#[derive(Debug, Default)]
struct ParsedFlags {
    saw_help: bool,
    dry_run: bool,
    params: Option<FlagArg>,
    json: Option<FlagArg>,
    upload_path: Option<String>,
    output_path: Option<String>,
    sanitize_template: Option<String>,
    unsupported_flags: Vec<String>,
    missing_value_flag: Option<String>,
    positionals: Vec<String>,
}

fn summarize_schema(argv: &[String]) -> crate::SummaryOutcome {
    let parsed = parse_flags(argv, 2, true);
    if let Some(flag) = parsed.missing_value_flag {
        return unknown_outcome(&format!("gws schema flag missing value: {flag}"));
    }
    if !parsed.unsupported_flags.is_empty() {
        return unknown_with_flags("unsupported gws schema flags", parsed.unsupported_flags);
    }
    if parsed.positionals.len() != 1 {
        return unknown_outcome("gws schema target required");
    }

    known_net_outcome(Action::Connect, GWS_API_ORIGIN, Vec::new())
}

fn summarize_service(argv: &[String]) -> crate::SummaryOutcome {
    let parsed = parse_flags(argv, 2, false);
    if let Some(flag) = parsed.missing_value_flag {
        return unknown_outcome(&format!("gws flag missing value: {flag}"));
    }
    if parsed.positionals.iter().any(|part| part.starts_with('+')) {
        return unknown_outcome("gws service helpers are unsupported");
    }
    if !parsed.unsupported_flags.is_empty() {
        return unknown_with_flags("unsupported gws flags", parsed.unsupported_flags);
    }
    if parsed.saw_help
        || parsed.positionals.is_empty()
        || parsed.positionals.iter().any(|part| part == "help")
    {
        return known_noop_outcome();
    }
    if parsed.positionals.len() < 2 {
        return known_noop_outcome();
    }

    let method = parsed
        .positionals
        .last()
        .expect("checked length above")
        .to_ascii_lowercase();

    if parsed.dry_run {
        return known_noop_outcome();
    }

    let primary_origin = if parsed.upload_path.is_some() {
        GWS_UPLOAD_ORIGIN
    } else {
        GWS_API_ORIGIN
    };

    let primary_action = if parsed.upload_path.is_some() || method_is_mutating(&method) {
        Action::Write
    } else {
        Action::Connect
    };

    let mut required_capabilities = vec![Capability {
        resource: Resource::Net,
        action: primary_action,
        scope: primary_origin.to_string(),
    }];

    if let Some(path) = parsed.upload_path {
        required_capabilities.push(Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: path,
        });
    }

    if let Some(path) = parsed.output_path {
        required_capabilities.push(Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: path,
        });
    }

    if parsed.sanitize_template.is_some() {
        required_capabilities.push(Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: GWS_MODELARMOR_ORIGIN.to_string(),
        });
    }

    let mut sink_checks = Vec::new();
    if primary_action == Action::Write {
        if let Some(flag) = parsed.params {
            sink_checks.push(net_sink_check(flag, primary_origin));
        }
        if let Some(flag) = parsed.json {
            sink_checks.push(net_sink_check(flag, primary_origin));
        }
    }

    known_outcome(CommandSummary {
        required_capabilities,
        sink_checks,
        unsupported_flags: Vec::new(),
    })
}

fn parse_flags(argv: &[String], start: usize, allow_resolve_refs: bool) -> ParsedFlags {
    let mut out = ParsedFlags::default();
    let mut saw_end_of_flags = false;
    let mut i = start;

    while i < argv.len() {
        let arg = &argv[i];
        if saw_end_of_flags {
            out.positionals.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            out.positionals.push(arg.clone());
            i += 1;
            continue;
        }

        if matches!(arg.as_str(), "-h" | "--help") {
            out.saw_help = true;
            i += 1;
            continue;
        }
        if arg == "--dry-run" {
            out.dry_run = true;
            i += 1;
            continue;
        }
        if arg == "--page-all" {
            i += 1;
            continue;
        }
        if allow_resolve_refs && arg == "--resolve-refs" {
            i += 1;
            continue;
        }

        if let Some(parsed) = parse_flag_value(arg, "-o", "--output", i, argv) {
            handle_string_flag(parsed, &mut out.output_path, &mut out.missing_value_flag);
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--upload", i, argv) {
            handle_string_flag(parsed, &mut out.upload_path, &mut out.missing_value_flag);
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--params", i, argv) {
            handle_tracked_flag(
                parsed,
                "--params",
                &mut out.params,
                &mut out.missing_value_flag,
            );
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--json", i, argv) {
            handle_tracked_flag(parsed, "--json", &mut out.json, &mut out.missing_value_flag);
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--sanitize", i, argv) {
            handle_string_flag(
                parsed,
                &mut out.sanitize_template,
                &mut out.missing_value_flag,
            );
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = consume_ignored_value_flag(arg, i, argv) {
            if let Err(flag) = parsed {
                out.missing_value_flag = Some(flag);
            }
            i += consumed_slots(arg);
            continue;
        }

        out.unsupported_flags.push(arg.clone());
        i += 1;
    }

    out
}

struct ParsedValue {
    value: String,
    value_index: usize,
}

fn parse_flag_value(
    arg: &str,
    short_flag: &str,
    long_flag: &str,
    i: usize,
    argv: &[String],
) -> Option<Result<ParsedValue, String>> {
    if let Some(value) = parse_attached_value(arg, short_flag, long_flag) {
        return Some(if value.is_empty() {
            Err(arg.to_string())
        } else {
            Ok(ParsedValue {
                value: value.to_string(),
                value_index: i,
            })
        });
    }
    if is_flag_name(arg, short_flag, long_flag) {
        return Some(
            argv.get(i + 1)
                .cloned()
                .map(|value| ParsedValue {
                    value,
                    value_index: i + 1,
                })
                .ok_or_else(|| arg.to_string()),
        );
    }
    None
}

fn handle_string_flag(
    parsed: Result<ParsedValue, String>,
    slot: &mut Option<String>,
    missing_value_flag: &mut Option<String>,
) {
    match parsed {
        Ok(parsed) => *slot = Some(parsed.value),
        Err(flag) => *missing_value_flag = Some(flag),
    }
}

fn handle_tracked_flag(
    parsed: Result<ParsedValue, String>,
    argument_name: &str,
    slot: &mut Option<FlagArg>,
    missing_value_flag: &mut Option<String>,
) {
    match parsed {
        Ok(parsed) => {
            *slot = Some(FlagArg {
                argument_name: argument_name.to_string(),
                value_index: parsed.value_index,
            });
        }
        Err(flag) => *missing_value_flag = Some(flag),
    }
}

fn consume_ignored_value_flag(arg: &str, i: usize, argv: &[String]) -> Option<Result<(), String>> {
    const VALUE_FLAGS: &[&str] = &["--format", "--api-version", "--page-limit", "--page-delay"];

    for flag in VALUE_FLAGS {
        if let Some(value) = parse_attached_value(arg, "", flag) {
            return Some(if value.is_empty() {
                Err(arg.to_string())
            } else {
                Ok(())
            });
        }
        if arg == *flag {
            return Some(if argv.get(i + 1).is_none() {
                Err(arg.to_string())
            } else {
                Ok(())
            });
        }
    }

    None
}

fn parse_attached_value<'a>(arg: &'a str, short_flag: &str, long_flag: &str) -> Option<&'a str> {
    if !short_flag.is_empty() && arg.starts_with(short_flag) && arg.len() > short_flag.len() {
        return Some(&arg[short_flag.len()..]);
    }
    if let Some(value) = arg.strip_prefix(&format!("{long_flag}=")) {
        return Some(value);
    }
    None
}

fn is_flag_name(arg: &str, short_flag: &str, long_flag: &str) -> bool {
    (!short_flag.is_empty() && arg == short_flag) || arg == long_flag
}

fn consumed_slots(arg: &str) -> usize {
    if arg.contains('=') || (arg.starts_with("-o") && arg.len() > 2) {
        1
    } else {
        2
    }
}

fn method_is_mutating(method: &str) -> bool {
    if matches!(
        method,
        "get" | "list" | "search" | "lookup" | "query" | "read" | "export"
    ) || method.starts_with("get")
        || method.starts_with("list")
        || method.starts_with("search")
        || method.starts_with("lookup")
        || method.starts_with("query")
        || method.starts_with("read")
        || method.starts_with("export")
        || method.starts_with("batchget")
    {
        return false;
    }

    matches!(
        method,
        "create"
            | "delete"
            | "update"
            | "patch"
            | "copy"
            | "watch"
            | "stop"
            | "send"
            | "append"
            | "insert"
            | "modify"
            | "move"
            | "accept"
            | "reject"
            | "cancel"
            | "archive"
            | "trash"
            | "untrash"
            | "close"
            | "open"
            | "undelete"
            | "subscribe"
            | "unsubscribe"
            | "enable"
            | "disable"
    ) || method.starts_with("create")
        || method.starts_with("delete")
        || method.starts_with("update")
        || method.starts_with("patch")
        || method.starts_with("copy")
        || method.starts_with("watch")
        || method.starts_with("stop")
        || method.starts_with("send")
        || method.starts_with("append")
        || method.starts_with("insert")
        || method.starts_with("modify")
        || method.starts_with("batchupdate")
        || method.starts_with("batchdelete")
        || method.starts_with("batchclear")
        || method.starts_with("move")
        || method.starts_with("accept")
        || method.starts_with("reject")
        || method.starts_with("cancel")
        || method.starts_with("archive")
        || method.starts_with("trash")
        || method.starts_with("untrash")
        || method.starts_with("close")
        || method.starts_with("open")
        || method.starts_with("undelete")
        || method.starts_with("subscribe")
        || method.starts_with("unsubscribe")
        || method.starts_with("enable")
        || method.starts_with("disable")
}

fn net_sink_check(flag: FlagArg, sink: &str) -> SinkCheck {
    SinkCheck {
        argument_name: flag.argument_name,
        sink: SinkKey(sink.to_string()),
        value_refs: vec![ValueRef(format!("argv:{}", flag.value_index))],
    }
}

fn known_noop_outcome() -> crate::SummaryOutcome {
    known_outcome(CommandSummary {
        required_capabilities: Vec::new(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn known_net_outcome(
    action: Action,
    scope: &str,
    mut extra_capabilities: Vec<Capability>,
) -> crate::SummaryOutcome {
    let mut required_capabilities = vec![Capability {
        resource: Resource::Net,
        action,
        scope: scope.to_string(),
    }];
    required_capabilities.append(&mut extra_capabilities);
    known_outcome(CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}
