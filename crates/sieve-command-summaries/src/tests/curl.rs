use super::argv;
use crate::summarize_argv;
use sieve_types::{Action, Capability, CommandKnowledge, Resource, SinkCheck, SinkKey, ValueRef};

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

#[test]
fn curl_get_with_header_extracts_sink_check() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-H",
        "Authorization: Bearer secret",
        "https://api.example.com/v1/resource",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://api.example.com/".to_string(),
        }]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "-H".to_string(),
            sink: SinkKey("https://api.example.com/v1/resource".to_string()),
            value_refs: vec![ValueRef("argv:2".to_string())],
        }]
    );
}

#[test]
fn curl_get_with_attached_short_header_extracts_sink_check() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-HAuthorization: Bearer secret",
        "https://api.example.com/v1/resource",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://api.example.com/".to_string(),
        }]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "-H".to_string(),
            sink: SinkKey("https://api.example.com/v1/resource".to_string()),
            value_refs: vec![ValueRef("argv:1".to_string())],
        }]
    );
}

#[test]
fn curl_get_with_user_extracts_sink_check() {
    let cases = [
        (
            vec![
                "curl",
                "-u",
                "user:secret",
                "https://api.example.com/v1/resource",
            ],
            "-u",
            "argv:2",
        ),
        (
            vec![
                "curl",
                "--user=user:secret",
                "https://api.example.com/v1/resource",
            ],
            "--user",
            "argv:1",
        ),
        (
            vec![
                "curl",
                "--user",
                "user:secret",
                "https://api.example.com/v1/resource",
            ],
            "--user",
            "argv:2",
        ),
        (
            vec![
                "curl",
                "-uuser:secret",
                "https://api.example.com/v1/resource",
            ],
            "-u",
            "argv:1",
        ),
    ];

    for (argv_raw, argument_name, value_ref) in cases {
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
        assert_eq!(
            summary.sink_checks,
            vec![SinkCheck {
                argument_name: argument_name.to_string(),
                sink: SinkKey("https://api.example.com/v1/resource".to_string()),
                value_refs: vec![ValueRef(value_ref.to_string())],
            }],
            "{argv_raw:?}"
        );
    }
}

#[test]
fn curl_get_with_metadata_values_extracts_sink_check() {
    let cases = [
        (
            vec![
                "curl",
                "-A",
                "test-agent",
                "https://api.example.com/v1/resource",
            ],
            "-A",
            "argv:2",
        ),
        (
            vec![
                "curl",
                "--user-agent=test-agent",
                "https://api.example.com/v1/resource",
            ],
            "--user-agent",
            "argv:1",
        ),
        (
            vec![
                "curl",
                "-Atest-agent",
                "https://api.example.com/v1/resource",
            ],
            "-A",
            "argv:1",
        ),
        (
            vec![
                "curl",
                "-e",
                "https://referrer.example/path",
                "https://api.example.com/v1/resource",
            ],
            "-e",
            "argv:2",
        ),
        (
            vec![
                "curl",
                "--referer=https://referrer.example/path",
                "https://api.example.com/v1/resource",
            ],
            "--referer",
            "argv:1",
        ),
        (
            vec![
                "curl",
                "-ehttps://referrer.example/path",
                "https://api.example.com/v1/resource",
            ],
            "-e",
            "argv:1",
        ),
    ];

    for (argv_raw, argument_name, value_ref) in cases {
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
        assert_eq!(
            summary.sink_checks,
            vec![SinkCheck {
                argument_name: argument_name.to_string(),
                sink: SinkKey("https://api.example.com/v1/resource".to_string()),
                value_refs: vec![ValueRef(value_ref.to_string())],
            }],
            "{argv_raw:?}"
        );
    }
}

#[test]
fn curl_get_with_bearer_token_extracts_sink_check() {
    for (argv_raw, value_ref) in [
        (
            vec![
                "curl",
                "--oauth2-bearer",
                "secret-token",
                "https://api.example.com/v1/resource",
            ],
            "argv:2",
        ),
        (
            vec![
                "curl",
                "--oauth2-bearer=secret-token",
                "https://api.example.com/v1/resource",
            ],
            "argv:1",
        ),
    ] {
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
        assert_eq!(
            summary.sink_checks,
            vec![SinkCheck {
                argument_name: "--oauth2-bearer".to_string(),
                sink: SinkKey("https://api.example.com/v1/resource".to_string()),
                value_refs: vec![ValueRef(value_ref.to_string())],
            }],
            "{argv_raw:?}"
        );
    }
}

#[test]
fn sudo_curl_header_sink_ref_uses_original_argv_index() {
    let out = summarize_argv(&argv(&[
        "sudo",
        "curl",
        "-H",
        "Authorization: Bearer secret",
        "https://api.example.com/v1/resource",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://api.example.com/".to_string(),
        }]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "-H".to_string(),
            sink: SinkKey("https://api.example.com/v1/resource".to_string()),
            value_refs: vec![ValueRef("argv:3".to_string())],
        }]
    );
}

#[test]
fn env_curl_header_sink_ref_uses_original_argv_index() {
    let out = summarize_argv(&argv(&[
        "env",
        "TOKEN=secret",
        "curl",
        "-H",
        "Authorization: Bearer secret",
        "https://api.example.com/v1/resource",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://api.example.com/".to_string(),
        }]
    );
    assert_eq!(
        summary.sink_checks,
        vec![SinkCheck {
            argument_name: "-H".to_string(),
            sink: SinkKey("https://api.example.com/v1/resource".to_string()),
            value_refs: vec![ValueRef("argv:4".to_string())],
        }]
    );
}

#[test]
fn curl_header_file_requires_fs_read_capability() {
    let out = summarize_argv(&argv(&[
        "curl",
        "--header=@headers.txt",
        "https://api.example.com/v1/resource",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.example.com/".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "headers.txt".to_string(),
            }
        ]
    );
    assert_eq!(summary.sink_checks[0].argument_name, "--header");
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
