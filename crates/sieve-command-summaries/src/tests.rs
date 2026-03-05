use super::*;
use sieve_types::{Action, Capability, CommandKnowledge, Resource};
use sieve_types::{SinkCheck, SinkKey, ValueRef};

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
fn cp_maps_destination_to_fs_write_capability() {
    let out = summarize_argv(&argv(&["cp", "a.txt", "b.txt"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "b.txt".to_string(),
        }]
    );
}

#[test]
fn mv_maps_source_and_destination_to_fs_write_capability() {
    let out = summarize_argv(&argv(&["mv", "a.txt", "b.txt"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "a.txt".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "b.txt".to_string(),
            }
        ]
    );
}

#[test]
fn mkdir_mode_and_parents_flags_are_supported() {
    let out = summarize_argv(&argv(&["mkdir", "-p", "-m", "755", "tmp/work"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "tmp/work".to_string(),
        }]
    );
}

#[test]
fn touch_with_time_flag_maps_to_fs_write() {
    let out = summarize_argv(&argv(&["touch", "-d", "2026-01-01", "file.txt"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "file.txt".to_string(),
        }]
    );
}

#[test]
fn chmod_maps_targets_to_fs_write() {
    let out = summarize_argv(&argv(&["chmod", "-R", "755", "bin", "out"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "bin".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "out".to_string(),
            }
        ]
    );
}

#[test]
fn chown_unsupported_flag_routes_to_unknown() {
    let out = summarize_argv(&argv(&[
        "chown",
        "--from=user:group",
        "root:root",
        "file.txt",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.unsupported_flags,
        vec!["--from=user:group".to_string()]
    );
}

#[test]
fn tee_append_maps_to_fs_append_capability() {
    let out = summarize_argv(&argv(&["tee", "-a", "audit.log"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Fs,
            action: Action::Append,
            scope: "audit.log".to_string(),
        }]
    );
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
fn curl_payload_without_explicit_method_defaults_to_post() {
    let out = summarize_argv(&argv(&[
        "curl",
        "https://api.example.com/v1/upload",
        "--data",
        "body",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.required_capabilities.len(), 1);
    assert_eq!(
        summary.required_capabilities[0].scope,
        "https://api.example.com/v1/upload".to_string()
    );
    assert_eq!(summary.sink_checks[0].argument_name, "--data");
}

#[test]
fn curl_put_with_payload_extracts_sink_check() {
    let out = summarize_argv(&argv(&[
        "curl",
        "--request",
        "put",
        "--url",
        "https://api.example.com/v1/upload",
        "--data-binary",
        "blob",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.sink_checks.len(), 1);
    assert_eq!(summary.sink_checks[0].argument_name, "--data-binary");
    assert_eq!(
        summary.sink_checks[0].sink,
        SinkKey("https://api.example.com/v1/upload".to_string())
    );
}

#[test]
fn curl_header_flag_missing_value_routes_to_unknown() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "https://api.example.com/v1/upload",
        "-H",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some("curl header flag missing value")
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
fn curl_short_upload_file_flag_routes_to_unknown() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "PUT",
        "-T",
        "payload.bin",
        "https://api.example.com/v1/upload",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.unsupported_flags, vec!["-T".to_string()]);
}

#[test]
fn curl_multipart_form_flag_routes_to_unknown() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "-F",
        "file=@payload.bin",
        "https://api.example.com/v1/upload",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.unsupported_flags, vec!["-F".to_string()]);
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
fn curl_post_ipv6_sink_keeps_brackets() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "https://[2001:DB8::1]:443/a/./b/../c",
        "-d",
        "body",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    let expected = "https://[2001:db8::1]/a/c".to_string();
    assert_eq!(summary.required_capabilities[0].scope, expected);
    assert_eq!(summary.sink_checks[0].sink, SinkKey(expected));
}

#[test]
fn curl_post_idn_host_is_normalized_to_ascii() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "https://BÜCHER.example/%C3%BCber",
        "-d",
        "body",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    let expected = "https://xn--bcher-kva.example/%C3%BCber".to_string();
    assert_eq!(summary.required_capabilities[0].scope, expected);
}

#[test]
fn curl_get_url_requires_net_connect_capability() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-sS",
        "https://api.open-meteo.com/v1/forecast?latitude=1&longitude=2",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.sink_checks, Vec::<SinkCheck>::new());
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://api.open-meteo.com/".to_string(),
        }]
    );
}

#[test]
fn curl_get_connect_scope_keeps_non_default_port() {
    let out = summarize_argv(&argv(&["curl", "https://example.com:8443/path?q=1"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://example.com:8443/".to_string(),
        }]
    );
}

#[test]
fn curl_get_missing_url_routes_to_unknown() {
    let out = summarize_argv(&argv(&["curl", "-sS"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(out.reason.as_deref(), Some("curl request missing URL"));
}

#[test]
fn curl_post_encoded_slash_stays_encoded() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "https://api.example.com/a%2fb?debug=1",
        "-d",
        "body",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    let expected = "https://api.example.com/a%2Fb".to_string();
    assert_eq!(summary.required_capabilities[0].scope, expected);
}

#[test]
fn curl_post_host_without_path_normalizes_to_root_path() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "https://api.example.com",
        "-d",
        "body",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities[0].scope,
        "https://api.example.com/".to_string()
    );
}

#[test]
fn curl_post_dot_segment_with_trailing_slash_is_preserved() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "POST",
        "https://api.example.com/a/b/../",
        "-d",
        "body",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities[0].scope,
        "https://api.example.com/a/".to_string()
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

#[test]
fn planner_command_catalog_includes_bravesearch_entry() {
    assert!(planner_command_catalog().iter().any(|entry| {
        entry.command == "bravesearch" && entry.description.contains("Search Brave index")
    }));
}

#[test]
fn planner_command_catalog_bravesearch_mentions_discovery_followup() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "bravesearch")
        .expect("bravesearch catalog entry");
    assert!(entry.description.contains("After discovery"));
    assert!(entry.description.contains("curl"));
}

#[test]
fn planner_command_catalog_curl_mentions_markdown_new() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "curl")
        .expect("curl catalog entry");
    assert!(entry.description.contains("markdown.new"));
}

#[test]
fn planner_command_catalog_includes_codex_exec_entry() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "codex")
        .expect("codex catalog entry");
    assert!(entry.description.contains("codex exec"));
}

#[test]
fn planner_command_catalog_codex_mentions_read_only_and_workspace_write() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "codex")
        .expect("codex catalog entry");
    assert!(entry.description.contains("--sandbox read-only"));
    assert!(entry.description.contains("--sandbox workspace-write"));
    assert!(entry.description.contains("--ephemeral"));
}

#[test]
fn planner_command_catalog_includes_sieve_lcm_cli_entry() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "sieve-lcm-cli")
        .expect("sieve-lcm-cli catalog entry");
    assert!(entry.description.contains("query --lane both"));
    assert!(entry.description.contains("expand --ref"));
}

#[test]
fn planner_command_catalog_includes_st_entry() {
    let entry = planner_command_catalog()
        .iter()
        .find(|entry| entry.command == "st")
        .expect("st catalog entry");
    assert!(entry.description.contains("st stt"));
    assert!(entry.description.contains("st tts"));
}

#[test]
fn sieve_lcm_cli_query_is_known_no_capabilities() {
    let out = summarize_argv(&argv(&[
        "sieve-lcm-cli",
        "query",
        "--lane",
        "both",
        "--query",
        "where do i live",
        "--json",
    ]));
    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert!(summary.required_capabilities.is_empty());
}

#[test]
fn sieve_lcm_cli_ingest_requires_fs_write_capability() {
    let out = summarize_argv(&argv(&[
        "sieve-lcm-cli",
        "ingest",
        "--db",
        "/tmp/memory.db",
        "--conversation",
        "global",
        "--role",
        "user",
        "--content",
        "hello",
    ]));
    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.required_capabilities.len(), 1);
    assert_eq!(summary.required_capabilities[0].resource, Resource::Fs);
    assert_eq!(summary.required_capabilities[0].action, Action::Write);
    assert_eq!(
        summary.required_capabilities[0].scope,
        "/tmp/memory.db".to_string()
    );
}
