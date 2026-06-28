use sieve_types::{Action, Capability, CommandSummary, Resource, SinkCheck, SinkKey, ValueRef};

use crate::{is_named_command, known_outcome, unknown_outcome, unknown_with_flags};

mod flags;

use flags::{parse_flags, FlagArg};

const GWS_API_ORIGIN: &str = "https://www.googleapis.com/";
const GWS_UPLOAD_ORIGIN: &str = "https://www.googleapis.com/upload/";
const GWS_MODELARMOR_ORIGIN: &str = "https://modelarmor.googleapis.com/";

pub(super) fn summarize_gws(argv: &[String]) -> Option<crate::SummaryOutcome> {
    if !is_named_command(argv, "gws") {
        return None;
    }

    if argv.len() < 2 {
        return Some(known_noop_outcome());
    }

    let root = argv[1].as_str();
    Some(match root {
        "-h" | "--help" | "help" | "version" | "--version" | "-V" => known_noop_outcome(),
        "schema" => summarize_schema(argv),
        "auth" => unknown_outcome("gws auth commands are unsupported"),
        "mcp" => unknown_outcome("gws mcp is unsupported"),
        "workflow" => unknown_outcome("gws workflow helpers are unsupported"),
        "modelarmor" => unknown_outcome("gws modelarmor helpers are unsupported"),
        other if other.starts_with('+') => unknown_outcome("gws top-level helpers are unsupported"),
        _ => summarize_service(argv),
    })
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

    let has_request_body = parsed.upload_path.is_some() || parsed.json.is_some();
    let primary_action = if has_request_body || method_is_mutating(&method) {
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
    if let Some(flag) = parsed.sanitize_template {
        sink_checks.push(net_sink_check(flag, GWS_MODELARMOR_ORIGIN));
    }

    known_outcome(CommandSummary {
        required_capabilities,
        sink_checks,
        unsupported_flags: Vec::new(),
    })
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
