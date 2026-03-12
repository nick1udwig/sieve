use super::argv;
use crate::summarize_argv;
use sieve_types::{
    Action, Capability, CommandKnowledge, Resource, SinkChannel, SinkCheck, SinkKey, ValueRef,
};

#[test]
fn agent_browser_open_requires_explicit_origin_connect_capability() {
    let out = summarize_argv(&argv(&["agent-browser", "open", "example.com"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://example.com/".to_string(),
        }]
    );
    assert!(summary.sink_checks.is_empty());
}

#[test]
fn agent_browser_open_headers_add_sink_check() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "open",
        "https://api.example.com/v1",
        "--headers",
        "{\"Authorization\":\"Bearer x\"}",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.required_capabilities.len(), 1);
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "--headers".to_string(),
            sink: SinkKey("https://api.example.com/".to_string()),
            channel: SinkChannel::Header,
            value_refs: vec![ValueRef("argv:4".to_string())],
        }]
    );
}

#[test]
fn agent_browser_open_profile_and_state_add_fs_caps() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "--profile",
        "/tmp/profile",
        "--state=/tmp/state.json",
        "open",
        "https://example.com",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://example.com/".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/profile".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/profile".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/state.json".to_string(),
            }
        ]
    );
}

#[test]
fn agent_browser_connect_port_requires_local_net_connect() {
    let out = summarize_argv(&argv(&["agent-browser", "connect", "9222"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "http://localhost:9222/".to_string(),
        }]
    );
}

#[test]
fn agent_browser_tab_new_blank_is_known_without_network() {
    let out = summarize_argv(&argv(&["agent-browser", "tab", "new"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert!(summary.required_capabilities.is_empty());
}

#[test]
fn agent_browser_tab_new_url_requires_connect() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "tab",
        "new",
        "https://docs.example.com/page",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://docs.example.com/".to_string(),
        }]
    );
}

#[test]
fn agent_browser_set_viewport_is_known_without_network() {
    let out = summarize_argv(&argv(&["agent-browser", "set", "viewport", "1280", "720"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert!(summary.required_capabilities.is_empty());
}

#[test]
fn agent_browser_cookies_set_with_url_extracts_sink_check() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "cookies",
        "set",
        "session_id",
        "secret",
        "--url",
        "https://app.example.com/login",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://app.example.com/".to_string(),
        }]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "value".to_string(),
            sink: SinkKey("https://app.example.com/".to_string()),
            channel: SinkChannel::Cookie,
            value_refs: vec![ValueRef("argv:4".to_string())],
        }]
    );
}

#[test]
fn agent_browser_diff_url_requires_both_origins() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "diff",
        "url",
        "https://staging.example.com",
        "https://prod.example.com",
        "--screenshot",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://staging.example.com/".to_string(),
            },
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://prod.example.com/".to_string(),
            }
        ]
    );
}

#[test]
fn agent_browser_record_start_with_url_requires_fs_write_and_connect() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "record",
        "start",
        "./demo.webm",
        "https://example.com",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "./demo.webm".to_string(),
            },
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://example.com/".to_string(),
            }
        ]
    );
}

#[test]
fn agent_browser_snapshot_routes_to_unknown_without_explicit_origin() {
    let out = summarize_argv(&argv(&["agent-browser", "snapshot", "-i"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some(
            "agent-browser page interaction requires an explicit origin; hidden browser-session state is unsupported"
        )
    );
}

#[test]
fn agent_browser_storage_set_routes_to_unknown_without_explicit_origin() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "storage",
        "local",
        "set",
        "token",
        "secret",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some(
            "agent-browser page interaction requires an explicit origin; hidden browser-session state is unsupported"
        )
    );
}

#[test]
fn agent_browser_provider_flag_routes_to_unknown_with_unsupported_flags() {
    let out = summarize_argv(&argv(&[
        "agent-browser",
        "-p",
        "ios",
        "open",
        "https://example.com",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.unsupported_flags, vec!["-p".to_string()]);
}
