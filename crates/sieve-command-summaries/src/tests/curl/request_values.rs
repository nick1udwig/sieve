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
fn curl_get_with_output_file_requires_fs_write_capability() {
    for argv_raw in [
        vec![
            "curl",
            "-o",
            "response.json",
            "https://api.example.com/v1/resource",
        ],
        vec![
            "curl",
            "--output=response.json",
            "https://api.example.com/v1/resource",
        ],
        vec![
            "curl",
            "-oresponse.json",
            "https://api.example.com/v1/resource",
        ],
    ] {
        let out = summarize_argv(&argv(&argv_raw));

        assert_eq!(out.knowledge, CommandKnowledge::Known, "{argv_raw:?}");
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
                    action: Action::Write,
                    scope: "response.json".to_string(),
                },
            ],
            "{argv_raw:?}"
        );
        assert_eq!(summary.sink_checks, Vec::<SinkCheck>::new(), "{argv_raw:?}");
    }
}

#[test]
fn curl_get_with_stdout_output_requires_only_net_connect() {
    let out = summarize_argv(&argv(&[
        "curl",
        "-o",
        "-",
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
