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
fn curl_attached_short_request_method_requires_net_write() {
    let out = summarize_argv(&argv(&["curl", "-XPOST", "https://api.example.com/v1"]));

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
fn curl_explicit_get_with_payload_extracts_sink_check() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-X",
        "GET",
        "https://api.example.com/v1/query",
        "-d",
        "token=secret",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: "https://api.example.com/v1/query".to_string(),
        }]
    );
    assert_eq!(summary.sink_checks.len(), 1);
    assert_eq!(summary.sink_checks[0].argument_name, "-d");
    assert_eq!(
        summary.sink_checks[0].sink,
        SinkKey("https://api.example.com/v1/query".to_string())
    );
    assert_eq!(
        summary.sink_checks[0].value_refs,
        vec![ValueRef("argv:5".to_string())]
    );
}

#[test]
fn curl_get_flags_with_payload_extract_sink_check() {
    for flag in ["-G", "--get"] {
        let out = summarize_argv(&argv(&[
            "curl",
            flag,
            "-d",
            "token=secret",
            "https://api.example.com/v1/query",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known, "flag: {flag}");
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1/query".to_string(),
            }],
            "flag: {flag}"
        );
        assert_eq!(
            summary.sink_checks,
            vec![SinkCheck {
                argument_name: "-d".to_string(),
                sink: SinkKey("https://api.example.com/v1/query".to_string()),
                value_refs: vec![ValueRef("argv:3".to_string())],
            }],
            "flag: {flag}"
        );
    }
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
fn curl_payload_file_requires_fs_read_capability() {
    let out = summarize_argv(&argv(&[
        "curl",
        "--data-binary",
        "@secret.txt",
        "https://api.example.com/v1/upload",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1/upload".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "secret.txt".to_string(),
            }
        ]
    );
    assert_eq!(summary.sink_checks[0].argument_name, "--data-binary");
}

#[test]
fn curl_data_urlencode_named_file_requires_fs_read_capability() {
    let out = summarize_argv(&argv(&[
        "curl",
        "--data-urlencode",
        "payload@secret.txt",
        "https://api.example.com/v1/upload",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1/upload".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "secret.txt".to_string(),
            }
        ]
    );
    assert_eq!(summary.sink_checks[0].argument_name, "--data-urlencode");
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
