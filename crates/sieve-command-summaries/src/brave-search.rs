use sieve_types::{Action, Capability, Resource};

use crate::{
    known_capabilities_outcome, known_empty_outcome as known_noop_outcome, SummaryOutcome,
};

#[path = "brave-search/flags.rs"]
mod flags;

use flags::{latest_optional_value, latest_value, parse_flags, ParsedFlags};

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

    let parsed = match parse_checked_flags(
        argv,
        2,
        &value_flags,
        &bool_flags,
        "bravesearch search flag missing value",
        "unsupported bravesearch search flags",
    ) {
        Ok(parsed) => parsed,
        Err(out) => return out,
    };

    let mut required_capabilities = vec![net_connect_capability(endpoint)];
    for flag in ["api-key-file", "config"] {
        if let Some(path) = latest_optional_value(&parsed, flag) {
            required_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: path,
            });
        }
    }
    known_capabilities_outcome(required_capabilities)
}

fn summarize_config(argv: &[String]) -> SummaryOutcome {
    if argv.len() < 3 {
        return known_noop_outcome();
    }

    match argv[2].as_str() {
        "help" | "-h" | "--help" => known_noop_outcome(),
        "show" | "get" => {
            if let Err(out) = parse_checked_flags(
                argv,
                3,
                &["config"],
                &[],
                "bravesearch config flag missing value",
                "unsupported bravesearch config flags",
            ) {
                return out;
            }
            known_noop_outcome()
        }
        "init" => {
            let parsed = match parse_checked_flags(
                argv,
                3,
                &["config"],
                &["force"],
                "bravesearch config init flag missing value",
                "unsupported bravesearch config init flags",
            ) {
                Ok(parsed) => parsed,
                Err(out) => return out,
            };
            let config_path = latest_value(&parsed, "config", DEFAULT_CONFIG_PATH);
            super::known_fs_outcome(vec![config_path], Action::Write)
        }
        "set" => {
            let parsed = match parse_checked_flags(
                argv,
                3,
                &["config"],
                &[],
                "bravesearch config set flag missing value",
                "unsupported bravesearch config set flags",
            ) {
                Ok(parsed) => parsed,
                Err(out) => return out,
            };
            if parsed.positionals.len() != 2 {
                return super::unknown_outcome("bravesearch config set missing key/value");
            }
            let config_path = latest_value(&parsed, "config", DEFAULT_CONFIG_PATH);
            super::known_fs_outcome(vec![config_path], Action::Write)
        }
        "paths" => {
            if let Err(out) = parse_checked_flags(
                argv,
                3,
                &["config", "api-key-file"],
                &[],
                "bravesearch config paths flag missing value",
                "unsupported bravesearch config paths flags",
            ) {
                return out;
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
            if let Err(out) = parse_checked_flags(
                argv,
                3,
                &["cache-dir"],
                &[],
                "bravesearch cache stats flag missing value",
                "unsupported bravesearch cache stats flags",
            ) {
                return out;
            }
            known_noop_outcome()
        }
        "clear" | "prune" => {
            let parsed = match parse_checked_flags(
                argv,
                3,
                &["cache-dir"],
                &[],
                "bravesearch cache flag missing value",
                "unsupported bravesearch cache flags",
            ) {
                Ok(parsed) => parsed,
                Err(out) => return out,
            };
            let cache_dir = latest_value(&parsed, "cache-dir", DEFAULT_CACHE_DIR);
            super::known_fs_outcome(vec![cache_dir], Action::Write)
        }
        _ => super::unknown_outcome("unknown bravesearch cache command"),
    }
}

fn parse_checked_flags(
    argv: &[String],
    start: usize,
    value_flags: &[&str],
    bool_flags: &[&str],
    missing_value_reason: &str,
    unsupported_reason: &str,
) -> Result<ParsedFlags, SummaryOutcome> {
    let parsed = parse_flags(argv, start, value_flags, bool_flags);
    if parsed.saw_help {
        return Err(known_noop_outcome());
    }
    if let Some(flag) = parsed.missing_value_flag.as_ref() {
        let reason = format!("{missing_value_reason}: {flag}");
        return Err(super::unknown_outcome(&reason));
    }
    if !parsed.unsupported_flags.is_empty() {
        return Err(super::unknown_with_flags(
            unsupported_reason,
            parsed.unsupported_flags,
        ));
    }
    Ok(parsed)
}

fn net_connect_capability(scope: &str) -> Capability {
    let scope = super::canonicalize_url_connect_scope(scope)
        .map(|sink| sink.0)
        .unwrap_or_else(|| scope.to_string());
    Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope,
    }
}

fn is_brave_search_command(argv: &[String]) -> bool {
    super::basename(argv.first()).is_some_and(|cmd| matches!(cmd, "bravesearch" | "brave-search"))
}

#[cfg(test)]
#[path = "brave-search/tests.rs"]
mod tests;
