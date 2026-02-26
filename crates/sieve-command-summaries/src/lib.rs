#![forbid(unsafe_code)]

use codex_shell_command::command_safety::is_dangerous_command::command_might_be_dangerous;
use codex_shell_command::command_safety::is_safe_command::is_known_safe_command;
use sieve_types::{Action, Capability, Resource, SinkCheck, SinkKey, ValueRef};
use sieve_types::{CommandKnowledge, CommandSummary};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryOutcome {
    pub knowledge: CommandKnowledge,
    pub summary: Option<CommandSummary>,
    pub reason: Option<String>,
}

pub trait CommandSummarizer: Send + Sync {
    fn summarize(&self, argv: &[String]) -> SummaryOutcome;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultCommandSummarizer;

impl CommandSummarizer for DefaultCommandSummarizer {
    fn summarize(&self, argv: &[String]) -> SummaryOutcome {
        summarize_argv(argv)
    }
}

fn summarize_argv(argv: &[String]) -> SummaryOutcome {
    if argv.is_empty() {
        return unknown_outcome("empty argv");
    }

    if let Some(outcome) = summarize_rm(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_curl(argv) {
        return outcome;
    }

    if is_known_safe_command(argv) {
        return known_outcome(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        });
    }

    if command_might_be_dangerous(argv) {
        return unknown_outcome("dangerous command class lacks explicit summary");
    }

    unknown_outcome("unknown command")
}

fn summarize_rm(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_rm_command(inner) {
        return None;
    }

    let mut recursive = false;
    let mut force = false;
    let mut saw_end_of_flags = false;
    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();

    for arg in inner.iter().skip(1) {
        if saw_end_of_flags {
            targets.push(arg.clone());
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            continue;
        }
        if arg.starts_with('-') {
            match arg.as_str() {
                "-r" | "-R" | "--recursive" => recursive = true,
                "-f" | "--force" => force = true,
                "-rf" | "-fr" => {
                    recursive = true;
                    force = true;
                }
                _ => unsupported_flags.push(arg.clone()),
            }
            continue;
        }
        targets.push(arg.clone());
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported rm flags",
            unsupported_flags,
        ));
    }

    if !(recursive && force) {
        return None;
    }

    if targets.is_empty() {
        targets.push("*".to_string());
    }

    Some(known_outcome(CommandSummary {
        required_capabilities: targets
            .into_iter()
            .map(|target| Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: target,
            })
            .collect(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    }))
}

fn summarize_curl(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_curl_command(argv) {
        return None;
    }

    #[derive(Debug, Clone)]
    struct PayloadArg {
        flag: String,
        value_index: usize,
    }

    let mut method: Option<String> = None;
    let mut url_index: Option<usize> = None;
    let mut payloads: Vec<PayloadArg> = Vec::new();
    let mut unsupported_flags: Vec<String> = Vec::new();
    let mut i = 1usize;
    let mut saw_end_of_flags = false;

    while i < argv.len() {
        let arg = &argv[i];
        if !saw_end_of_flags && arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }

        if !saw_end_of_flags && arg.starts_with('-') {
            if arg == "-X" || arg == "--request" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl method flag missing value"));
                }
                method = Some(argv[i + 1].to_ascii_uppercase());
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--request=") {
                method = Some(raw.to_ascii_uppercase());
                i += 1;
                continue;
            }

            if arg == "-d"
                || arg == "--data"
                || arg == "--data-raw"
                || arg == "--data-binary"
                || arg == "--data-ascii"
                || arg == "--data-urlencode"
                || arg == "--json"
            {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl payload flag missing value"));
                }
                payloads.push(PayloadArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                });
                i += 2;
                continue;
            }

            if let Some((flag, _value)) = split_flag_value(arg) {
                if matches!(
                    flag,
                    "--data"
                        | "--data-raw"
                        | "--data-binary"
                        | "--data-ascii"
                        | "--data-urlencode"
                        | "--json"
                ) {
                    payloads.push(PayloadArg {
                        flag: flag.to_string(),
                        value_index: i,
                    });
                    i += 1;
                    continue;
                }
            }

            if matches!(
                arg.as_str(),
                "-s" | "-S" | "-L" | "-k" | "--silent" | "--show-error"
            ) || arg.starts_with("--header=")
                || arg == "-H"
            {
                if arg == "-H" && i + 1 < argv.len() {
                    i += 2;
                    continue;
                }
                i += 1;
                continue;
            }

            unsupported_flags.push(arg.clone());
            i += 1;
            continue;
        }

        if url_index.is_none() {
            url_index = Some(i);
        }
        i += 1;
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported curl flags",
            unsupported_flags,
        ));
    }

    let method = method.unwrap_or_else(|| "GET".to_string());
    if method != "POST" {
        return None;
    }

    let Some(url_idx) = url_index else {
        return Some(unknown_outcome("curl POST missing URL"));
    };
    let Some(sink) = canonicalize_url_sink(&argv[url_idx]) else {
        return Some(unknown_outcome("curl POST has invalid URL sink"));
    };

    let sink_checks = payloads
        .into_iter()
        .map(|payload| SinkCheck {
            argument_name: payload.flag,
            sink: sink.clone(),
            value_refs: vec![ValueRef(format!("argv:{}", payload.value_index))],
        })
        .collect();

    Some(known_outcome(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: sink.0.clone(),
        }],
        sink_checks,
        unsupported_flags: Vec::new(),
    }))
}

fn split_flag_value(flag: &str) -> Option<(&str, &str)> {
    let eq = flag.find('=')?;
    Some((&flag[..eq], &flag[eq + 1..]))
}

fn is_rm_command(argv: &[String]) -> bool {
    basename(argv.first()).is_some_and(|cmd| cmd == "rm")
}

fn is_curl_command(argv: &[String]) -> bool {
    basename(argv.first()).is_some_and(|cmd| cmd == "curl")
}

fn strip_sudo(argv: &[String]) -> &[String] {
    if basename(argv.first()).is_some_and(|cmd| cmd == "sudo") && argv.len() > 1 {
        &argv[1..]
    } else {
        argv
    }
}

fn basename(s: Option<&String>) -> Option<&str> {
    let s = s?;
    std::path::Path::new(s)
        .file_name()
        .and_then(|part| part.to_str())
}

fn canonicalize_url_sink(raw: &str) -> Option<SinkKey> {
    let url = Url::parse(raw).ok()?;
    let scheme = url.scheme().to_ascii_lowercase();
    let host = url.host_str()?.to_ascii_lowercase();
    let port = url
        .port()
        .filter(|p| Some(*p) != default_port_for_scheme(&scheme));
    let path = normalize_path(url.path());

    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(&path);
    Some(SinkKey(out))
}

fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn normalize_path(path: &str) -> String {
    let has_trailing_slash = path.ends_with('/') && path != "/";
    let mut stack: Vec<String> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            _ => stack.push(normalize_percent_encoding(segment)),
        }
    }

    if stack.is_empty() {
        return "/".to_string();
    }

    let mut out = format!("/{}", stack.join("/"));
    if has_trailing_slash {
        out.push('/');
    }
    out
}

fn normalize_percent_encoding(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(decoded) = decode_hex_pair(bytes[i + 1], bytes[i + 2]) {
                if is_unreserved(decoded) {
                    out.push(decoded as char);
                } else {
                    out.push('%');
                    out.push(to_upper_hex(decoded >> 4));
                    out.push(to_upper_hex(decoded & 0x0f));
                }
                i += 3;
                continue;
            }
        }

        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn decode_hex_pair(high: u8, low: u8) -> Option<u8> {
    Some((from_hex(high)? << 4) | from_hex(low)?)
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn to_upper_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => unreachable!("nibble out of range"),
    }
}

fn is_unreserved(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
    )
}

fn known_outcome(summary: CommandSummary) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Known,
        summary: Some(summary),
        reason: None,
    }
}

fn unknown_outcome(reason: &str) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Unknown,
        summary: None,
        reason: Some(reason.to_string()),
    }
}

fn unknown_with_flags(reason: &str, unsupported_flags: Vec<String>) -> SummaryOutcome {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn rm_rf_maps_to_fs_write_capability() {
        let out = summarize_argv(&argv(&["rm", "-rf", "/tmp/demo"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.required_capabilities.len(), 1);
        assert_eq!(
            summary.required_capabilities[0],
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/demo".to_string()
            }
        );
        assert!(summary.sink_checks.is_empty());
    }

    #[test]
    fn rm_unknown_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&["rm", "-rfv", "/tmp/demo"]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out
            .summary
            .expect("expected summary with unsupported flags");
        assert_eq!(summary.unsupported_flags, vec!["-rfv".to_string()]);
    }

    #[test]
    fn curl_post_url_requires_net_write_no_payload_sink_checks() {
        let out = summarize_argv(&argv(&["curl", "-X", "POST", "https://api.example.com/v1"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1".to_string()
            }]
        );
        assert!(summary.sink_checks.is_empty());
    }

    #[test]
    fn curl_post_with_payload_extracts_sink_check() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://api.example.com/v1/upload",
            "-d",
            "{\"k\":\"v\"}",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.sink_checks.len(), 1);
        assert_eq!(summary.sink_checks[0].argument_name, "-d");
        assert_eq!(
            summary.sink_checks[0].sink,
            SinkKey("https://api.example.com/v1/upload".to_string())
        );
        assert_eq!(
            summary.sink_checks[0].value_refs,
            vec![ValueRef("argv:5".to_string())]
        );
    }

    #[test]
    fn safe_read_command_is_known_with_empty_summary() {
        let out = summarize_argv(&argv(&["ls", "-la"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert!(summary.required_capabilities.is_empty());
        assert!(summary.sink_checks.is_empty());
        assert!(summary.unsupported_flags.is_empty());
    }

    #[test]
    fn curl_unknown_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "--upload-file",
            "payload.bin",
            "https://api.example.com/v1/upload",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.unsupported_flags, vec!["--upload-file".to_string()]);
    }

    #[test]
    fn curl_post_url_sink_is_canonicalized() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "HTTPS://API.Example.COM:443/a/../b/./c%7e?x=1#frag",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        let expected = "https://api.example.com/b/c~".to_string();
        assert_eq!(summary.required_capabilities[0].scope, expected);
        assert_eq!(summary.sink_checks[0].sink, SinkKey(expected));
    }

    #[test]
    fn curl_post_non_default_port_is_preserved() {
        let out = summarize_argv(&argv(&[
            "curl",
            "--request=POST",
            "https://api.example.com:8443/v1/upload",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities[0].scope,
            "https://api.example.com:8443/v1/upload".to_string()
        );
    }

    #[test]
    fn codex_safe_bash_lc_class_is_known() {
        let out = summarize_argv(&argv(&["bash", "-lc", "ls && cat Cargo.toml"]));
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert!(summary.required_capabilities.is_empty());
    }

    #[test]
    fn codex_dangerous_bash_lc_class_routes_to_unknown() {
        let out = summarize_argv(&argv(&["bash", "-lc", "rm -rf /tmp/demo"]));
        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(
            out.reason.as_deref(),
            Some("dangerous command class lacks explicit summary")
        );
    }

    #[test]
    fn rm_f_routes_to_dangerous_unknown() {
        let out = summarize_argv(&argv(&["rm", "-f", "/tmp/demo"]));
        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(
            out.reason.as_deref(),
            Some("dangerous command class lacks explicit summary")
        );
    }
}
