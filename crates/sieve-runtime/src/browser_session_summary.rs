use sieve_command_summaries::SummaryOutcome;
use sieve_types::{
    Action, Capability, CommandKnowledge, CommandSummary, Resource, SinkCheck, SinkKey, ValueRef,
};
use url::Url;

mod parse;

pub(crate) use parse::{parse_agent_browser, ParsedAgentBrowser};

const SNAPSHOT_VALUE_FLAGS: &[&str] = &["--depth", "--selector"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionTransition {
    None,
    SetCurrentPage { origin: String, url: String },
    Clear,
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

    let mut outcome = match command {
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
    };
    add_global_effects(&mut outcome, parsed, current_origin);
    Some(outcome)
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
        if let Some((_, value)) = split_supported_value_flag(arg, SNAPSHOT_VALUE_FLAGS) {
            if value.is_empty() {
                return unknown(&format!("agent-browser snapshot flag missing value: {arg}"));
            }
            i += 1;
            continue;
        }
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

fn split_supported_value_flag<'a>(arg: &'a str, flags: &[&str]) -> Option<(&'a str, &'a str)> {
    let (flag, value) = arg.split_once('=')?;
    flags.contains(&flag).then_some((flag, value))
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

fn add_global_effects(
    outcome: &mut SummaryOutcome,
    parsed: &ParsedAgentBrowser,
    current_origin: &str,
) {
    if outcome.knowledge != CommandKnowledge::Known {
        return;
    }
    let Some(summary) = &mut outcome.summary else {
        return;
    };
    for capability in &parsed.global_capabilities {
        push_capability(
            &mut summary.required_capabilities,
            capability.resource,
            capability.action,
            capability.scope.clone(),
        );
    }
    for index in &parsed.header_value_indices {
        summary.sink_checks.push(SinkCheck {
            argument_name: "--headers".to_string(),
            sink: SinkKey(current_origin.to_string()),
            value_refs: vec![ValueRef(format!("argv:{index}"))],
        });
    }
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
