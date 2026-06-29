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
fn curl_get_with_safe_transport_flags_requires_net_connect_capability() {
    for flag in [
        "--compressed",
        "--no-progress-meter",
        "--http1.1",
        "--http2",
        "--ipv4",
        "--ipv6",
        "-4",
        "-6",
    ] {
        let out = summarize_argv(&argv(&[
            "curl",
            flag,
            "https://api.example.com/v1/resource",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known, "flag: {flag}");
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.sink_checks, Vec::<SinkCheck>::new(), "flag: {flag}");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.example.com/".to_string(),
            }],
            "flag: {flag}"
        );
    }
}

#[test]
fn curl_get_with_safe_value_flags_requires_net_connect_capability() {
    let cases = [
        vec![
            "curl",
            "--connect-timeout",
            "3",
            "https://api.example.com/v1/resource",
        ],
        vec![
            "curl",
            "--max-time=5",
            "https://api.example.com/v1/resource",
        ],
        vec!["curl", "-m", "5", "https://api.example.com/v1/resource"],
        vec!["curl", "-m5", "https://api.example.com/v1/resource"],
        vec![
            "curl",
            "--retry",
            "2",
            "https://api.example.com/v1/resource",
        ],
        vec![
            "curl",
            "--retry-delay=1",
            "https://api.example.com/v1/resource",
        ],
        vec![
            "curl",
            "--retry-max-time",
            "10",
            "https://api.example.com/v1/resource",
        ],
        vec![
            "curl",
            "--max-redirs",
            "3",
            "https://api.example.com/v1/resource",
        ],
    ];

    for argv_raw in cases {
        let out = summarize_argv(&argv(&argv_raw));

        assert_eq!(out.knowledge, CommandKnowledge::Known, "{argv_raw:?}");
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.example.com/".to_string(),
            }],
            "{argv_raw:?}"
        );
        assert_eq!(summary.sink_checks, Vec::<SinkCheck>::new(), "{argv_raw:?}");
    }
}

#[test]
fn curl_head_flags_require_net_connect_capability() {
    for flag in ["-I", "--head"] {
        let out = summarize_argv(&argv(&[
            "curl",
            flag,
            "https://api.example.com/v1/resource",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known, "flag: {flag}");
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.sink_checks, Vec::<SinkCheck>::new());
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.example.com/".to_string(),
            }],
            "flag: {flag}"
        );
    }
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
