use sieve_types::{
    Action, Capability, CommandSummary, Resource, SinkChannel, SinkCheck, SinkKey, ValueRef,
};

use crate::{canonicalize_url_connect_scope, SummaryOutcome};

const PAGE_CONTEXT_REASON: &str =
    "agent-browser page interaction requires an explicit origin; hidden browser-session state is unsupported";
const UNSUPPORTED_GLOBAL_FLAGS_REASON: &str = "unsupported agent-browser global flags";
const UNSUPPORTED_COMMAND_FLAGS_REASON: &str = "unsupported agent-browser flags";
const GLOBAL_HEADER_REASON: &str =
    "agent-browser --headers requires a single explicit navigation origin";

pub(super) fn summarize_agent_browser(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = super::strip_sudo(argv);
    if super::basename(inner.first()).is_none_or(|cmd| cmd != "agent-browser") {
        return None;
    }

    let parsed = parse_globals(inner);
    if let Some(flag) = parsed.missing_value_flag {
        return Some(super::unknown_outcome(&format!(
            "agent-browser global flag missing value: {flag}"
        )));
    }
    if !parsed.unsupported_flags.is_empty() {
        return Some(super::unknown_with_flags(
            UNSUPPORTED_GLOBAL_FLAGS_REASON,
            parsed.unsupported_flags,
        ));
    }

    let Some(command) = parsed.remaining.first() else {
        return Some(known_empty_outcome());
    };

    Some(match command.value.as_str() {
        "help" | "-h" | "--help" | "version" | "--version" | "-V" => known_empty_outcome(),
        "open" | "goto" | "navigate" => summarize_open(&parsed),
        "connect" => summarize_connect(&parsed),
        "tab" => summarize_tab(&parsed),
        "set" => summarize_set(&parsed),
        "cookies" => summarize_cookies(&parsed),
        "diff" => summarize_diff(&parsed),
        "record" => summarize_record(&parsed),
        "close" | "quit" | "exit" => finalize_known(&parsed, Vec::new(), Vec::new(), None),
        "session" => summarize_session(&parsed),
        "confirm" | "deny" => summarize_confirmation(&parsed),
        "snapshot" | "click" | "dblclick" | "type" | "fill" | "press" | "key" | "keydown"
        | "keyup" | "keyboard" | "hover" | "focus" | "check" | "uncheck" | "select" | "drag"
        | "upload" | "download" | "scroll" | "scrollintoview" | "scrollinto" | "wait"
        | "screenshot" | "pdf" | "get" | "is" | "find" | "mouse" | "back" | "forward"
        | "reload" | "frame" | "dialog" | "storage" | "network" | "trace" | "profiler"
        | "console" | "errors" | "highlight" | "auth" | "install" => {
            super::unknown_outcome(PAGE_CONTEXT_REASON)
        }
        _ => super::unknown_outcome("unsupported agent-browser command"),
    })
}

#[derive(Debug, Clone)]
struct Token {
    index: usize,
    value: String,
}

#[derive(Debug, Default)]
struct ParsedGlobals {
    remaining: Vec<Token>,
    browser_capabilities: Vec<Capability>,
    header_value_indices: Vec<usize>,
    unsupported_flags: Vec<String>,
    missing_value_flag: Option<String>,
}

fn parse_globals(argv: &[String]) -> ParsedGlobals {
    let mut out = ParsedGlobals::default();
    let mut i = 1usize;

    while i < argv.len() {
        let arg = &argv[i];

        if matches!(arg.as_str(), "--json" | "--headed" | "--debug") {
            i += 1;
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--session") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            i += parsed.consumed();
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--profile") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            let path = parsed.value();
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Read,
                path.to_string(),
            );
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Write,
                path.to_string(),
            );
            i += parsed.consumed();
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--state") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Read,
                parsed.value().to_string(),
            );
            i += parsed.consumed();
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--config") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Read,
                parsed.value().to_string(),
            );
            i += parsed.consumed();
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--extension") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Read,
                parsed.value().to_string(),
            );
            i += parsed.consumed();
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--download-path") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Write,
                parsed.value().to_string(),
            );
            i += parsed.consumed();
            continue;
        }

        if let Some(parsed) = parse_value_flag(argv, i, "--action-policy") {
            if parsed.is_missing() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            push_capability(
                &mut out.browser_capabilities,
                Resource::Fs,
                Action::Read,
                parsed.value().to_string(),
            );
            i += parsed.consumed();
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
        if let Some((flag, value)) = super::split_flag_value(arg) {
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
    let (parsed_flag, value) = super::split_flag_value(arg)?;
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

fn summarize_open(parsed: &ParsedGlobals) -> SummaryOutcome {
    if parsed.remaining.len() != 2 {
        return super::unknown_outcome("agent-browser open requires exactly one URL");
    }
    let Some(origin) = connect_scope_from_navigation(&parsed.remaining[1].value) else {
        return super::unknown_outcome("agent-browser open has invalid URL");
    };
    finalize_known(
        parsed,
        net_connect_caps(std::slice::from_ref(&origin)),
        Vec::new(),
        Some(origin),
    )
}

fn summarize_connect(parsed: &ParsedGlobals) -> SummaryOutcome {
    if !parsed.header_value_indices.is_empty() {
        return super::unknown_outcome(GLOBAL_HEADER_REASON);
    }
    if parsed.remaining.len() != 2 {
        return super::unknown_outcome("agent-browser connect requires exactly one port or URL");
    }
    let Some(origin) = connect_scope_from_connect_target(&parsed.remaining[1].value) else {
        return super::unknown_outcome("agent-browser connect target is unsupported");
    };
    finalize_known(parsed, net_connect_caps(&[origin]), Vec::new(), None)
}

fn summarize_tab(parsed: &ParsedGlobals) -> SummaryOutcome {
    let Some(op) = parsed.remaining.get(1).map(|token| token.value.as_str()) else {
        return super::unknown_outcome(PAGE_CONTEXT_REASON);
    };
    match op {
        "new" => {
            if parsed.remaining.len() == 2 {
                return finalize_known(parsed, Vec::new(), Vec::new(), None);
            }
            if parsed.remaining.len() != 3 {
                return super::unknown_outcome("agent-browser tab new accepts at most one URL");
            }
            let Some(origin) = connect_scope_from_navigation(&parsed.remaining[2].value) else {
                return super::unknown_outcome("agent-browser tab new has invalid URL");
            };
            finalize_known(
                parsed,
                net_connect_caps(std::slice::from_ref(&origin)),
                Vec::new(),
                Some(origin),
            )
        }
        _ => super::unknown_outcome(PAGE_CONTEXT_REASON),
    }
}

fn summarize_set(parsed: &ParsedGlobals) -> SummaryOutcome {
    if !parsed.header_value_indices.is_empty() {
        return super::unknown_outcome(GLOBAL_HEADER_REASON);
    }
    let Some(setting) = parsed.remaining.get(1).map(|token| token.value.as_str()) else {
        return super::unknown_outcome("agent-browser set missing setting");
    };
    match setting {
        "viewport" if parsed.remaining.len() == 4 => {
            finalize_known(parsed, Vec::new(), Vec::new(), None)
        }
        "device" if parsed.remaining.len() == 3 => {
            finalize_known(parsed, Vec::new(), Vec::new(), None)
        }
        "geo" if parsed.remaining.len() == 4 => {
            finalize_known(parsed, Vec::new(), Vec::new(), None)
        }
        "offline" if matches!(parsed.remaining.len(), 2 | 3) => {
            finalize_known(parsed, Vec::new(), Vec::new(), None)
        }
        "media" if matches!(parsed.remaining.len(), 3 | 4) => {
            finalize_known(parsed, Vec::new(), Vec::new(), None)
        }
        "headers" | "credentials" => super::unknown_outcome(PAGE_CONTEXT_REASON),
        _ => super::unknown_outcome("unsupported agent-browser set command"),
    }
}

fn summarize_cookies(parsed: &ParsedGlobals) -> SummaryOutcome {
    let op = parsed
        .remaining
        .get(1)
        .map(|token| token.value.as_str())
        .unwrap_or("get");
    match op {
        "set" => summarize_cookies_set(parsed),
        _ => super::unknown_outcome(PAGE_CONTEXT_REASON),
    }
}

fn summarize_cookies_set(parsed: &ParsedGlobals) -> SummaryOutcome {
    if parsed.remaining.len() < 4 {
        return super::unknown_outcome("agent-browser cookies set requires name and value");
    }

    let mut url: Option<SinkKey> = None;
    let mut unsupported_flags = Vec::new();
    let mut i = 4usize;

    while i < parsed.remaining.len() {
        let arg = parsed.remaining[i].value.as_str();
        match arg {
            "--httpOnly" | "--secure" => i += 1,
            "--url" | "--domain" | "--path" | "--sameSite" | "--expires" => {
                let Some(value) = parsed.remaining.get(i + 1) else {
                    return super::unknown_outcome(&format!(
                        "agent-browser cookies set flag missing value: {arg}"
                    ));
                };
                if arg == "--url" {
                    url = connect_scope_from_navigation(&value.value);
                    if url.is_none() {
                        return super::unknown_outcome(
                            "agent-browser cookies set has invalid --url",
                        );
                    }
                }
                i += 2;
            }
            _ if arg.starts_with("--url=") => {
                let raw = &arg["--url=".len()..];
                if raw.is_empty() {
                    return super::unknown_outcome(
                        "agent-browser cookies set flag missing value: --url=",
                    );
                }
                url = connect_scope_from_navigation(raw);
                if url.is_none() {
                    return super::unknown_outcome("agent-browser cookies set has invalid --url");
                }
                i += 1;
            }
            _ if matches!(
                super::split_flag_value(arg),
                Some(("--domain" | "--path" | "--sameSite" | "--expires", value)) if !value.is_empty()
            ) =>
            {
                i += 1;
            }
            _ if matches!(
                super::split_flag_value(arg),
                Some(("--domain" | "--path" | "--sameSite" | "--expires", _))
            ) =>
            {
                return super::unknown_outcome(&format!(
                    "agent-browser cookies set flag missing value: {arg}"
                ));
            }
            _ => {
                unsupported_flags.push(parsed.remaining[i].value.clone());
                i += 1;
            }
        }
    }

    if !unsupported_flags.is_empty() {
        return super::unknown_with_flags(UNSUPPORTED_COMMAND_FLAGS_REASON, unsupported_flags);
    }

    let Some(origin) = url else {
        return super::unknown_outcome(PAGE_CONTEXT_REASON);
    };
    let cookie_value_index = parsed.remaining[3].index;
    finalize_known(
        parsed,
        net_connect_caps(std::slice::from_ref(&origin)),
        vec![SinkCheck {
            argument_name: "value".to_string(),
            sink: origin.clone(),
            channel: SinkChannel::Cookie,
            value_refs: vec![ValueRef(format!("argv:{cookie_value_index}"))],
        }],
        Some(origin),
    )
}

fn summarize_diff(parsed: &ParsedGlobals) -> SummaryOutcome {
    let Some(mode) = parsed.remaining.get(1).map(|token| token.value.as_str()) else {
        return super::unknown_outcome("agent-browser diff missing mode");
    };
    match mode {
        "url" => summarize_diff_url(parsed),
        _ => super::unknown_outcome(PAGE_CONTEXT_REASON),
    }
}

fn summarize_diff_url(parsed: &ParsedGlobals) -> SummaryOutcome {
    if parsed.remaining.len() < 4 {
        return super::unknown_outcome("agent-browser diff url requires two URLs");
    }
    if !parsed.header_value_indices.is_empty() {
        return super::unknown_outcome(GLOBAL_HEADER_REASON);
    }
    let Some(left) = connect_scope_from_navigation(&parsed.remaining[2].value) else {
        return super::unknown_outcome("agent-browser diff url has invalid first URL");
    };
    let Some(right) = connect_scope_from_navigation(&parsed.remaining[3].value) else {
        return super::unknown_outcome("agent-browser diff url has invalid second URL");
    };
    let mut unsupported_flags = Vec::new();
    let mut i = 4usize;
    while i < parsed.remaining.len() {
        let arg = parsed.remaining[i].value.as_str();
        match arg {
            "--screenshot" | "--full" | "-c" | "--compact" => i += 1,
            "--wait-until" | "-s" | "--selector" | "-d" | "--depth" => {
                if parsed.remaining.get(i + 1).is_none() {
                    return super::unknown_outcome(&format!(
                        "agent-browser diff url flag missing value: {arg}"
                    ));
                }
                i += 2;
            }
            _ => {
                unsupported_flags.push(parsed.remaining[i].value.clone());
                i += 1;
            }
        }
    }
    if !unsupported_flags.is_empty() {
        return super::unknown_with_flags(UNSUPPORTED_COMMAND_FLAGS_REASON, unsupported_flags);
    }
    finalize_known(parsed, net_connect_caps(&[left, right]), Vec::new(), None)
}

fn summarize_record(parsed: &ParsedGlobals) -> SummaryOutcome {
    let Some(op) = parsed.remaining.get(1).map(|token| token.value.as_str()) else {
        return super::unknown_outcome("agent-browser record missing operation");
    };
    match op {
        "start" | "restart" => summarize_record_with_optional_url(parsed),
        _ => super::unknown_outcome(PAGE_CONTEXT_REASON),
    }
}

fn summarize_record_with_optional_url(parsed: &ParsedGlobals) -> SummaryOutcome {
    if parsed.remaining.len() < 3 {
        return super::unknown_outcome("agent-browser record start requires an output path");
    }
    let output_path = parsed.remaining[2].value.clone();
    if parsed.remaining.len() == 3 {
        return super::unknown_outcome(PAGE_CONTEXT_REASON);
    }
    if parsed.remaining.len() != 4 {
        return super::unknown_outcome("agent-browser record start accepts at most one URL");
    }
    let Some(origin) = connect_scope_from_navigation(&parsed.remaining[3].value) else {
        return super::unknown_outcome("agent-browser record start has invalid URL");
    };
    finalize_known(
        parsed,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: output_path,
            },
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: origin.0.clone(),
            },
        ],
        Vec::new(),
        Some(origin),
    )
}

fn summarize_session(parsed: &ParsedGlobals) -> SummaryOutcome {
    if !parsed.header_value_indices.is_empty() {
        return super::unknown_outcome(GLOBAL_HEADER_REASON);
    }
    if parsed.remaining.len() <= 2
        && parsed
            .remaining
            .get(1)
            .is_none_or(|token| token.value == "list")
    {
        return known_empty_outcome();
    }
    super::unknown_outcome("unsupported agent-browser session command")
}

fn summarize_confirmation(parsed: &ParsedGlobals) -> SummaryOutcome {
    if !parsed.header_value_indices.is_empty() {
        return super::unknown_outcome(GLOBAL_HEADER_REASON);
    }
    if parsed.remaining.len() == 2 {
        return known_empty_outcome();
    }
    super::unknown_outcome("agent-browser confirmation command requires exactly one id")
}

fn finalize_known(
    parsed: &ParsedGlobals,
    mut capabilities: Vec<Capability>,
    mut sink_checks: Vec<SinkCheck>,
    header_origin: Option<SinkKey>,
) -> SummaryOutcome {
    if !parsed.header_value_indices.is_empty() {
        let Some(origin) = header_origin else {
            return super::unknown_outcome(GLOBAL_HEADER_REASON);
        };
        for idx in &parsed.header_value_indices {
            sink_checks.push(SinkCheck {
                argument_name: "--headers".to_string(),
                sink: origin.clone(),
                channel: SinkChannel::Header,
                value_refs: vec![ValueRef(format!("argv:{idx}"))],
            });
        }
    }

    for capability in &parsed.browser_capabilities {
        push_capability(
            &mut capabilities,
            capability.resource,
            capability.action,
            capability.scope.clone(),
        );
    }

    super::known_outcome(CommandSummary {
        required_capabilities: capabilities,
        sink_checks,
        unsupported_flags: Vec::new(),
    })
}

fn known_empty_outcome() -> SummaryOutcome {
    super::known_outcome(CommandSummary {
        required_capabilities: Vec::new(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn connect_scope_from_navigation(raw: &str) -> Option<SinkKey> {
    if raw.contains("://") {
        return canonicalize_url_connect_scope(raw);
    }
    canonicalize_url_connect_scope(&format!("https://{raw}"))
}

fn connect_scope_from_connect_target(raw: &str) -> Option<SinkKey> {
    if raw.chars().all(|ch| ch.is_ascii_digit()) {
        return canonicalize_url_connect_scope(&format!("http://localhost:{raw}"));
    }
    canonicalize_url_connect_scope(raw)
}

fn net_connect_caps(origins: &[SinkKey]) -> Vec<Capability> {
    let mut out = Vec::new();
    for origin in origins {
        push_capability(&mut out, Resource::Net, Action::Connect, origin.0.clone());
    }
    out
}

fn push_capability(
    capabilities: &mut Vec<Capability>,
    resource: Resource,
    action: Action,
    scope: String,
) {
    let capability = Capability {
        resource,
        action,
        scope,
    };
    if !capabilities.contains(&capability) {
        capabilities.push(capability);
    }
}
