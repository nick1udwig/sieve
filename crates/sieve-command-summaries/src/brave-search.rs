use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::SummaryOutcome;

const DEFAULT_CONFIG_PATH: &str = "~/.brave-search/config.json";
const DEFAULT_CACHE_DIR: &str = "~/.brave-search/cache";
const WEB_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const NEWS_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/news/search";
const IMAGES_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/images/search";
const VIDEOS_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/videos/search";

pub(super) fn summarize_brave_search(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_brave_search_command(argv) {
        return None;
    }

    if argv.len() < 2 {
        return Some(known_noop_outcome());
    }

    let root = argv[1].as_str();
    Some(match root {
        "help" | "-h" | "--help" | "version" => known_noop_outcome(),
        "search" => summarize_search(argv, WEB_SEARCH_ENDPOINT, SearchFlags::web()),
        "news" => summarize_search(argv, NEWS_SEARCH_ENDPOINT, SearchFlags::news()),
        "images" => summarize_search(argv, IMAGES_SEARCH_ENDPOINT, SearchFlags::images()),
        "videos" => summarize_search(argv, VIDEOS_SEARCH_ENDPOINT, SearchFlags::videos()),
        "config" => summarize_config(argv),
        "cache" => summarize_cache(argv),
        _ => super::unknown_outcome("unknown bravesearch command"),
    })
}

#[derive(Debug, Clone, Copy)]
struct SearchFlags {
    extra_snippets: bool,
    summary: bool,
    rich_callback: bool,
    result_filter: bool,
    goggle: bool,
}

impl SearchFlags {
    fn web() -> Self {
        Self {
            extra_snippets: true,
            summary: true,
            rich_callback: true,
            result_filter: true,
            goggle: true,
        }
    }

    fn news() -> Self {
        Self {
            extra_snippets: true,
            summary: false,
            rich_callback: false,
            result_filter: false,
            goggle: true,
        }
    }

    fn videos() -> Self {
        Self {
            extra_snippets: false,
            summary: false,
            rich_callback: false,
            result_filter: false,
            goggle: false,
        }
    }

    fn images() -> Self {
        Self {
            extra_snippets: false,
            summary: false,
            rich_callback: false,
            result_filter: false,
            goggle: false,
        }
    }
}

fn summarize_search(argv: &[String], endpoint: &str, flags: SearchFlags) -> SummaryOutcome {
    let mut bool_flags = vec!["spellcheck", "no-cache", "refresh"];
    let mut value_flags = vec![
        "q",
        "query",
        "count",
        "offset",
        "country",
        "search-lang",
        "ui-lang",
        "safesearch",
        "freshness",
        "param",
        "api-key",
        "api-key-file",
        "config",
        "cache-ttl",
        "timeout",
        "api-version",
        "max-retries",
        "output",
    ];
    if flags.extra_snippets {
        bool_flags.push("extra-snippets");
    }
    if flags.summary {
        bool_flags.push("summary");
    }
    if flags.rich_callback {
        bool_flags.push("enable-rich-callback");
    }
    if flags.result_filter {
        value_flags.push("result-filter");
    }
    if flags.goggle {
        value_flags.push("goggle");
    }

    let parsed = parse_flags(argv, 2, &value_flags, &bool_flags);
    if parsed.saw_help {
        return known_noop_outcome();
    }
    if let Some(flag) = parsed.missing_value_flag {
        let reason = format!("bravesearch search flag missing value: {flag}");
        return super::unknown_outcome(&reason);
    }
    if !parsed.unsupported_flags.is_empty() {
        return super::unknown_with_flags(
            "unsupported bravesearch search flags",
            parsed.unsupported_flags,
        );
    }

    known_net_connect_outcome(endpoint)
}

fn summarize_config(argv: &[String]) -> SummaryOutcome {
    if argv.len() < 3 {
        return known_noop_outcome();
    }

    match argv[2].as_str() {
        "help" | "-h" | "--help" => known_noop_outcome(),
        "show" | "get" => {
            let parsed = parse_flags(argv, 3, &["config"], &[]);
            if parsed.saw_help {
                return known_noop_outcome();
            }
            if let Some(flag) = parsed.missing_value_flag {
                let reason = format!("bravesearch config flag missing value: {flag}");
                return super::unknown_outcome(&reason);
            }
            if !parsed.unsupported_flags.is_empty() {
                return super::unknown_with_flags(
                    "unsupported bravesearch config flags",
                    parsed.unsupported_flags,
                );
            }
            known_noop_outcome()
        }
        "init" => {
            let parsed = parse_flags(argv, 3, &["config"], &["force"]);
            if parsed.saw_help {
                return known_noop_outcome();
            }
            if let Some(flag) = parsed.missing_value_flag {
                let reason = format!("bravesearch config init flag missing value: {flag}");
                return super::unknown_outcome(&reason);
            }
            if !parsed.unsupported_flags.is_empty() {
                return super::unknown_with_flags(
                    "unsupported bravesearch config init flags",
                    parsed.unsupported_flags,
                );
            }
            let config_path = latest_value(&parsed, "config", DEFAULT_CONFIG_PATH);
            super::known_fs_outcome(vec![config_path], Action::Write)
        }
        "set" => {
            let parsed = parse_flags(argv, 3, &["config"], &[]);
            if parsed.saw_help {
                return known_noop_outcome();
            }
            if let Some(flag) = parsed.missing_value_flag {
                let reason = format!("bravesearch config set flag missing value: {flag}");
                return super::unknown_outcome(&reason);
            }
            if !parsed.unsupported_flags.is_empty() {
                return super::unknown_with_flags(
                    "unsupported bravesearch config set flags",
                    parsed.unsupported_flags,
                );
            }
            if parsed.positionals.len() != 2 {
                return super::unknown_outcome("bravesearch config set missing key/value");
            }
            let config_path = latest_value(&parsed, "config", DEFAULT_CONFIG_PATH);
            super::known_fs_outcome(vec![config_path], Action::Write)
        }
        "paths" => {
            let parsed = parse_flags(argv, 3, &["config", "api-key-file"], &[]);
            if parsed.saw_help {
                return known_noop_outcome();
            }
            if let Some(flag) = parsed.missing_value_flag {
                let reason = format!("bravesearch config paths flag missing value: {flag}");
                return super::unknown_outcome(&reason);
            }
            if !parsed.unsupported_flags.is_empty() {
                return super::unknown_with_flags(
                    "unsupported bravesearch config paths flags",
                    parsed.unsupported_flags,
                );
            }
            known_noop_outcome()
        }
        _ => super::unknown_outcome("unknown bravesearch config command"),
    }
}

fn summarize_cache(argv: &[String]) -> SummaryOutcome {
    if argv.len() < 3 {
        return known_noop_outcome();
    }

    match argv[2].as_str() {
        "help" | "-h" | "--help" => known_noop_outcome(),
        "stats" => {
            let parsed = parse_flags(argv, 3, &["cache-dir"], &[]);
            if parsed.saw_help {
                return known_noop_outcome();
            }
            if let Some(flag) = parsed.missing_value_flag {
                let reason = format!("bravesearch cache stats flag missing value: {flag}");
                return super::unknown_outcome(&reason);
            }
            if !parsed.unsupported_flags.is_empty() {
                return super::unknown_with_flags(
                    "unsupported bravesearch cache stats flags",
                    parsed.unsupported_flags,
                );
            }
            known_noop_outcome()
        }
        "clear" | "prune" => {
            let parsed = parse_flags(argv, 3, &["cache-dir"], &[]);
            if parsed.saw_help {
                return known_noop_outcome();
            }
            if let Some(flag) = parsed.missing_value_flag {
                let reason = format!("bravesearch cache flag missing value: {flag}");
                return super::unknown_outcome(&reason);
            }
            if !parsed.unsupported_flags.is_empty() {
                return super::unknown_with_flags(
                    "unsupported bravesearch cache flags",
                    parsed.unsupported_flags,
                );
            }
            let cache_dir = latest_value(&parsed, "cache-dir", DEFAULT_CACHE_DIR);
            super::known_fs_outcome(vec![cache_dir], Action::Write)
        }
        _ => super::unknown_outcome("unknown bravesearch cache command"),
    }
}

#[derive(Debug, Default)]
struct ParsedFlags {
    unsupported_flags: Vec<String>,
    positionals: Vec<String>,
    values: Vec<(String, String)>,
    saw_help: bool,
    missing_value_flag: Option<String>,
}

fn parse_flags(
    argv: &[String],
    start: usize,
    value_flags: &[&str],
    bool_flags: &[&str],
) -> ParsedFlags {
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

        let Some((name, inline_value)) = parse_flag_token(arg) else {
            out.positionals.push(arg.clone());
            i += 1;
            continue;
        };

        if matches!(name, "h" | "help") {
            out.saw_help = true;
            i += 1;
            continue;
        }

        if bool_flags.contains(&name) {
            i += 1;
            continue;
        }

        if value_flags.contains(&name) {
            if let Some(value) = inline_value {
                if value.is_empty() {
                    out.missing_value_flag = Some(arg.clone());
                    break;
                }
                out.values.push((name.to_string(), value.to_string()));
                i += 1;
                continue;
            }
            if i + 1 >= argv.len() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.values.push((name.to_string(), argv[i + 1].clone()));
            i += 2;
            continue;
        }

        out.unsupported_flags.push(arg.clone());
        i += 1;
    }

    out
}

fn parse_flag_token(arg: &str) -> Option<(&str, Option<&str>)> {
    if !arg.starts_with('-') || arg == "-" {
        return None;
    }

    let trimmed = arg.trim_start_matches('-');
    if trimmed.is_empty() {
        return None;
    }

    if let Some((name, value)) = trimmed.split_once('=') {
        return Some((name, Some(value)));
    }
    Some((trimmed, None))
}

fn latest_value(parsed: &ParsedFlags, key: &str, default: &str) -> String {
    parsed
        .values
        .iter()
        .rev()
        .find_map(|(k, v)| if k == key { Some(v.clone()) } else { None })
        .unwrap_or_else(|| default.to_string())
}

fn known_net_connect_outcome(scope: &str) -> SummaryOutcome {
    let scope = super::canonicalize_url_connect_scope(scope)
        .map(|sink| sink.0)
        .unwrap_or_else(|| scope.to_string());
    super::known_outcome(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope,
        }],
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn known_noop_outcome() -> SummaryOutcome {
    super::known_outcome(CommandSummary {
        required_capabilities: Vec::new(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn is_brave_search_command(argv: &[String]) -> bool {
    super::basename(argv.first()).is_some_and(|cmd| matches!(cmd, "bravesearch" | "brave-search"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::CommandKnowledge;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn web_search_maps_to_net_connect_capability() {
        let out = crate::summarize_argv(&argv(&["bravesearch", "search", "--q", "rust"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.search.brave.com/".to_string(),
            }]
        );
        assert!(summary.sink_checks.is_empty());
    }

    #[test]
    fn brave_search_alias_is_supported() {
        let out = crate::summarize_argv(&argv(&["brave-search", "news", "--q", "rust"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities[0],
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.search.brave.com/".to_string(),
            }
        );
    }

    #[test]
    fn unsupported_search_flag_routes_to_unknown() {
        let out = crate::summarize_argv(&argv(&[
            "bravesearch",
            "search",
            "--upload-file",
            "payload.bin",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.unsupported_flags, vec!["--upload-file".to_string()]);
    }

    #[test]
    fn search_help_routes_to_known_noop() {
        let out = crate::summarize_argv(&argv(&["bravesearch", "search", "--help"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert!(summary.required_capabilities.is_empty());
    }

    #[test]
    fn config_init_requires_fs_write_on_default_path() {
        let out = crate::summarize_argv(&argv(&["bravesearch", "config", "init"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: DEFAULT_CONFIG_PATH.to_string(),
            }]
        );
    }

    #[test]
    fn config_set_requires_fs_write_on_config_override() {
        let out = crate::summarize_argv(&argv(&[
            "bravesearch",
            "config",
            "set",
            "--config",
            "/tmp/brave.json",
            "default_count",
            "10",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/brave.json".to_string(),
            }]
        );
    }

    #[test]
    fn config_set_without_key_value_routes_to_unknown() {
        let out = crate::summarize_argv(&argv(&["bravesearch", "config", "set"]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(
            out.reason.as_deref(),
            Some("bravesearch config set missing key/value")
        );
    }

    #[test]
    fn cache_clear_requires_fs_write_on_default_cache_dir() {
        let out = crate::summarize_argv(&argv(&["bravesearch", "cache", "clear"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: DEFAULT_CACHE_DIR.to_string(),
            }]
        );
    }
}
