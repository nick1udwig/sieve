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
