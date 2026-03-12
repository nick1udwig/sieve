use sieve_command_summaries::SummaryOutcome;
use sieve_types::{
    Action, Capability, CommandKnowledge, CommandSummary, Resource, SinkChannel, SinkCheck,
    SinkKey, ValueRef,
};
use std::path::Path;
use url::Url;

#[derive(Debug, Clone)]
pub(crate) struct ParsedAgentBrowser {
    pub(crate) remaining: Vec<Token>,
    pub(crate) session_name: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub(crate) index: usize,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionTransition {
    None,
    SetCurrentPage { origin: String, url: String },
    Clear,
}

pub(crate) fn parse_agent_browser(argv: &[String]) -> Option<ParsedAgentBrowser> {
    let inner = strip_sudo(argv);
    if basename(inner.first()) != Some("agent-browser") {
        return None;
    }

    let mut remaining = Vec::new();
    let mut session_name = None;
    let mut i = 1usize;
    while i < inner.len() {
        let arg = &inner[i];
        if matches!(arg.as_str(), "--json" | "--headed" | "--debug") {
            i += 1;
            continue;
        }
        if let Some((value, consumed)) = parse_value_flag(inner, i, "--session") {
            session_name = Some(value.to_string());
            i += consumed;
            continue;
        }
        if should_skip_value_flag(inner, i) {
            i += if arg.contains('=') { 1 } else { 2 };
            continue;
        }
        if should_skip_bool_flag(arg) {
            i += 1;
            continue;
        }
        remaining.push(Token {
            index: i,
            value: arg.clone(),
        });
        i += 1;
    }

    Some(ParsedAgentBrowser {
        remaining,
        session_name,
    })
}

pub(crate) fn session_transition(parsed: &ParsedAgentBrowser) -> SessionTransition {
    let Some(command) = parsed.remaining.first().map(|token| token.value.as_str()) else {
        return SessionTransition::None;
    };
    match command {
        "open" | "goto" | "navigate" if parsed.remaining.len() == 2 => {
            navigation_transition(&parsed.remaining[1].value)
        }
        "tab"
            if parsed
                .remaining
                .get(1)
                .is_some_and(|token| token.value == "new")
                && parsed.remaining.len() == 3 =>
        {
            navigation_transition(&parsed.remaining[2].value)
        }
        "record"
            if parsed
                .remaining
                .get(1)
                .is_some_and(|token| matches!(token.value.as_str(), "start" | "restart"))
                && parsed.remaining.len() == 4 =>
        {
            navigation_transition(&parsed.remaining[3].value)
        }
        "close" | "quit" | "exit" => SessionTransition::Clear,
        _ => SessionTransition::None,
    }
}

pub(crate) fn contextual_summary(
    parsed: &ParsedAgentBrowser,
    current_origin: &str,
) -> Option<SummaryOutcome> {
    let connect_cap = Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: current_origin.to_string(),
    };
    let sink = SinkKey(current_origin.to_string());
    let command = parsed.remaining.first()?.value.as_str();

    Some(match command {
        "snapshot" => parse_snapshot(parsed, connect_cap),
        "get" => parse_get(parsed, connect_cap),
        "is" => parse_is(parsed, connect_cap),
        "click" | "dblclick" | "hover" | "focus" | "check" | "uncheck" | "select" | "drag"
        | "scroll" | "scrollintoview" | "scrollinto" | "mouse" => {
            let Some(ctx) = require_min_args(parsed, 2) else {
                return Some(unknown("agent-browser action missing target"));
            };
            ctx.or_known(vec![connect_cap], Vec::new())
        }
        "fill" | "type" => parse_text_input(parsed, connect_cap, sink),
        "keyboard" => parse_keyboard(parsed, connect_cap, sink),
        "screenshot" => parse_screenshot(parsed, connect_cap),
        "pdf" => parse_pdf(parsed, connect_cap),
        "download" => parse_download(parsed, connect_cap),
        "upload" => parse_upload(parsed, connect_cap),
        "storage" => parse_storage(parsed, connect_cap, sink),
        _ => return None,
    })
}

fn navigation_transition(raw: &str) -> SessionTransition {
    let Some(url) = normalize_navigation_url(raw) else {
        return SessionTransition::None;
    };
    let Some(origin) = connect_scope_from_navigation(raw) else {
        return SessionTransition::None;
    };
    SessionTransition::SetCurrentPage {
        origin,
        url: url.to_string(),
    }
}

#[derive(Debug)]
struct ParsedContext {
    caps: Vec<Capability>,
    sink_checks: Vec<SinkCheck>,
}

impl ParsedContext {
    fn or_known(
        self,
        mut caps: Vec<Capability>,
        mut sink_checks: Vec<SinkCheck>,
    ) -> SummaryOutcome {
        caps.extend(self.caps);
        sink_checks.extend(self.sink_checks);
        known(caps, sink_checks)
    }
}

fn require_min_args(parsed: &ParsedAgentBrowser, count: usize) -> Option<ParsedContext> {
    (parsed.remaining.len() >= count).then_some(ParsedContext {
        caps: Vec::new(),
        sink_checks: Vec::new(),
    })
}

fn parse_snapshot(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    let mut i = 1usize;
    while i < parsed.remaining.len() {
        let arg = parsed.remaining[i].value.as_str();
        match arg {
            "-i" | "-C" | "-c" | "--interactive" | "--cursor" | "--compact" => i += 1,
            "-d" | "--depth" | "-s" | "--selector" => {
                if parsed.remaining.get(i + 1).is_none() {
                    return unknown(&format!("agent-browser snapshot flag missing value: {arg}"));
                }
                i += 2;
            }
            _ => return unknown(&format!("unsupported contextual agent-browser flag: {arg}")),
        }
    }
    known(vec![connect_cap], Vec::new())
}

fn parse_get(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    if parsed.remaining.len() < 2 {
        return unknown("agent-browser get missing subcommand");
    }
    match parsed.remaining[1].value.as_str() {
        "text" | "html" | "value" | "count" | "box" | "styles" if parsed.remaining.len() == 3 => {
            known(vec![connect_cap], Vec::new())
        }
        "attr" if parsed.remaining.len() == 4 => known(vec![connect_cap], Vec::new()),
        "title" | "url" if parsed.remaining.len() == 2 => known(vec![connect_cap], Vec::new()),
        other => unknown(&format!(
            "unsupported contextual agent-browser get command: {other}"
        )),
    }
}

fn parse_is(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    if parsed.remaining.len() == 3
        && matches!(
            parsed.remaining[1].value.as_str(),
            "visible" | "enabled" | "checked"
        )
    {
        return known(vec![connect_cap], Vec::new());
    }
    unknown("unsupported contextual agent-browser is command")
}

fn parse_text_input(
    parsed: &ParsedAgentBrowser,
    connect_cap: Capability,
    sink: SinkKey,
) -> SummaryOutcome {
    if parsed.remaining.len() != 3 {
        return unknown("agent-browser text input requires selector and text");
    }
    known(
        vec![connect_cap],
        vec![SinkCheck {
            argument_name: parsed.remaining[0].value.clone(),
            sink,
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef(format!("argv:{}", parsed.remaining[2].index))],
        }],
    )
}

fn parse_keyboard(
    parsed: &ParsedAgentBrowser,
    connect_cap: Capability,
    sink: SinkKey,
) -> SummaryOutcome {
    if parsed.remaining.len() != 3 {
        return unknown("agent-browser keyboard requires subcommand and text");
    }
    if !matches!(parsed.remaining[1].value.as_str(), "type" | "inserttext") {
        return unknown("unsupported contextual agent-browser keyboard command");
    }
    known(
        vec![connect_cap],
        vec![SinkCheck {
            argument_name: format!("keyboard {}", parsed.remaining[1].value),
            sink,
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef(format!("argv:{}", parsed.remaining[2].index))],
        }],
    )
}

fn parse_screenshot(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    let mut path: Option<String> = None;
    let mut i = 1usize;
    while i < parsed.remaining.len() {
        let arg = parsed.remaining[i].value.as_str();
        match arg {
            "--full" | "-f" | "--annotate" => i += 1,
            _ if arg.starts_with('-') => {
                return unknown(&format!("unsupported contextual agent-browser flag: {arg}"));
            }
            _ => {
                if path.is_some() {
                    return unknown("agent-browser screenshot accepts at most one path");
                }
                path = Some(parsed.remaining[i].value.clone());
                i += 1;
            }
        }
    }
    let Some(path) = path else {
        return unknown("agent-browser screenshot without explicit path is unsupported");
    };
    known(
        vec![
            connect_cap,
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: path,
            },
        ],
        Vec::new(),
    )
}

fn parse_pdf(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    if parsed.remaining.len() != 2 {
        return unknown("agent-browser pdf requires an output path");
    }
    known(
        vec![
            connect_cap,
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: parsed.remaining[1].value.clone(),
            },
        ],
        Vec::new(),
    )
}

fn parse_download(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    if parsed.remaining.len() != 3 {
        return unknown("agent-browser download requires selector and path");
    }
    known(
        vec![
            connect_cap,
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: parsed.remaining[2].value.clone(),
            },
        ],
        Vec::new(),
    )
}

fn parse_upload(parsed: &ParsedAgentBrowser, connect_cap: Capability) -> SummaryOutcome {
    if parsed.remaining.len() < 3 {
        return unknown("agent-browser upload requires selector and files");
    }
    let mut caps = vec![connect_cap];
    for token in parsed.remaining.iter().skip(2) {
        caps.push(Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: token.value.clone(),
        });
    }
    known(caps, Vec::new())
}

fn parse_storage(
    parsed: &ParsedAgentBrowser,
    connect_cap: Capability,
    sink: SinkKey,
) -> SummaryOutcome {
    if parsed.remaining.len() < 2
        || !matches!(parsed.remaining[1].value.as_str(), "local" | "session")
    {
        return unknown("unsupported contextual agent-browser storage command");
    }
    if parsed.remaining.len() == 2 {
        return known(vec![connect_cap], Vec::new());
    }
    match parsed.remaining[2].value.as_str() {
        "get" if matches!(parsed.remaining.len(), 3 | 4) => known(vec![connect_cap], Vec::new()),
        "clear" if parsed.remaining.len() == 3 => known(vec![connect_cap], Vec::new()),
        "set" if parsed.remaining.len() == 5 => known(
            vec![connect_cap],
            vec![SinkCheck {
                argument_name: "storage.set".to_string(),
                sink,
                channel: SinkChannel::Body,
                value_refs: vec![ValueRef(format!("argv:{}", parsed.remaining[4].index))],
            }],
        ),
        _ => unknown("unsupported contextual agent-browser storage command"),
    }
}

fn known(required_capabilities: Vec<Capability>, sink_checks: Vec<SinkCheck>) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Known,
        summary: Some(CommandSummary {
            required_capabilities,
            sink_checks,
            unsupported_flags: Vec::new(),
        }),
        reason: None,
    }
}

fn unknown(reason: &str) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Unknown,
        summary: None,
        reason: Some(reason.to_string()),
    }
}

fn strip_sudo(argv: &[String]) -> &[String] {
    if basename(argv.first()) == Some("sudo") && argv.len() > 1 {
        &argv[1..]
    } else {
        argv
    }
}

fn basename(s: Option<&String>) -> Option<&str> {
    let s = s?;
    Path::new(s).file_name().and_then(|part| part.to_str())
}

fn parse_value_flag<'a>(argv: &'a [String], i: usize, flag: &str) -> Option<(&'a str, usize)> {
    let arg = argv.get(i)?;
    if arg == flag {
        return argv.get(i + 1).map(|value| (value.as_str(), 2));
    }
    let (parsed_flag, value) = arg.split_once('=')?;
    (parsed_flag == flag && !value.is_empty()).then_some((value, 1))
}

fn should_skip_value_flag(argv: &[String], i: usize) -> bool {
    const VALUE_FLAGS: &[&str] = &[
        "--profile",
        "--state",
        "--config",
        "--extension",
        "--download-path",
        "--action-policy",
        "--headers",
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

fn normalize_navigation_url(raw: &str) -> Option<Url> {
    if raw.contains("://") {
        return Url::parse(raw).ok();
    }
    Url::parse(&format!("https://{raw}")).ok()
}

fn connect_scope_from_navigation(raw: &str) -> Option<String> {
    let url = normalize_navigation_url(raw)?;
    let host = url.host_str()?.to_ascii_lowercase();
    let mut out = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        let default = match url.scheme() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        if Some(port) != default {
            out.push(':');
            out.push_str(&port.to_string());
        }
    }
    out.push('/');
    Some(out)
}
