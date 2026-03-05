use super::*;
use sieve_types::CommandKnowledge;

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn codex_exec_read_only_requires_ephemeral() {
    let out = crate::summarize_argv(&argv(&["codex", "exec", "--sandbox", "read-only"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some("codex exec read-only requires --ephemeral")
    );
}

#[test]
fn codex_exec_read_only_with_ephemeral_is_known() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "exec",
        "--sandbox",
        "read-only",
        "--ephemeral",
        "analyze this repo",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: CODEX_API_CONNECT_SCOPE.to_string(),
        }]
    );
    assert!(summary.sink_checks.is_empty());
    assert!(summary.unsupported_flags.is_empty());
}

#[test]
fn codex_exec_read_only_accepts_image_flag() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "exec",
        "--sandbox=read-only",
        "--ephemeral",
        "--image",
        "/tmp/ui.png",
        "read this image",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(summary.required_capabilities.len(), 1);
    assert_eq!(summary.required_capabilities[0].resource, Resource::Net);
    assert_eq!(summary.required_capabilities[0].action, Action::Connect);
    assert_eq!(
        summary.required_capabilities[0].scope,
        CODEX_API_CONNECT_SCOPE.to_string()
    );
}

#[test]
fn codex_exec_workspace_write_requires_cd_or_cwd_and_add_dir_scopes() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "exec",
        "--sandbox",
        "workspace-write",
        "--cd",
        "/repo",
        "--add-dir",
        "/tmp/scratch",
        "fix this bug",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: CODEX_API_CONNECT_SCOPE.to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/repo".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/scratch".to_string(),
            }
        ]
    );
}

#[test]
fn codex_exec_workspace_write_defaults_to_cwd_scope() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "exec",
        "--sandbox=workspace-write",
        "write changes",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: CODEX_API_CONNECT_SCOPE.to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: ".".to_string(),
            }
        ]
    );
}

#[test]
fn codex_exec_read_only_forbids_output_last_message_file() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "exec",
        "--sandbox",
        "read-only",
        "--ephemeral",
        "--output-last-message",
        "/tmp/out.txt",
        "summarize",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    assert_eq!(
        out.reason.as_deref(),
        Some("codex exec read-only forbids --output-last-message")
    );
}

#[test]
fn codex_exec_unknown_flag_routes_to_unknown() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "exec",
        "--sandbox",
        "workspace-write",
        "--not-a-real-flag",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.unsupported_flags,
        vec!["--not-a-real-flag".to_string()]
    );
}

#[test]
fn codex_exec_alias_e_is_supported() {
    let out = crate::summarize_argv(&argv(&[
        "codex",
        "e",
        "--sandbox",
        "workspace-write",
        "-C",
        "/repo",
        "refactor",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: CODEX_API_CONNECT_SCOPE.to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/repo".to_string(),
            }
        ]
    );
}
