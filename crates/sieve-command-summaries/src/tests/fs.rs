use super::argv;
use crate::summarize_argv;
use sieve_types::{Action, Capability, CommandKnowledge, Resource};

#[test]
fn trash_maps_targets_to_fs_write_capability() {
    let out = summarize_argv(&argv(&["trash", "/tmp/demo"]));

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
fn trash_custom_trash_dir_adds_fs_write_capability() {
    let out = summarize_argv(&argv(&[
        "trash",
        "--trash-dir",
        "/tmp/custom-trash",
        "/tmp/demo",
    ]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/demo".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/custom-trash".to_string(),
            }
        ]
    );
}

#[test]
fn trash_unknown_flag_routes_to_unknown() {
    let out = summarize_argv(&argv(&["trash", "--bogus", "/tmp/demo"]));

    assert_eq!(out.knowledge, CommandKnowledge::Unknown);
    let summary = out
        .summary
        .expect("expected summary with unsupported flags");
    assert_eq!(summary.unsupported_flags, vec!["--bogus".to_string()]);
}

#[test]
fn trash_print_completion_is_metadata_only() {
    let out = summarize_argv(&argv(&["trash", "--print-completion=bash"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert!(summary.required_capabilities.is_empty());
    assert!(summary.sink_checks.is_empty());
}

#[test]
fn cp_maps_source_read_and_destination_write_capabilities() {
    let out = summarize_argv(&argv(&["cp", "a.txt", "b.txt"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
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
fn mkdir_end_of_flags_treats_dash_prefixed_operand_as_target() {
    let out = summarize_argv(&argv(&["mkdir", "--", "-p"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "-p".to_string(),
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
fn touch_end_of_flags_treats_dash_prefixed_operand_as_target() {
    let out = summarize_argv(&argv(&["touch", "--", "-d"]));

    assert_eq!(out.knowledge, CommandKnowledge::Known);
    let summary = out.summary.expect("expected summary");
    assert_eq!(
        summary.required_capabilities,
        vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "-d".to_string(),
        }]
    );
}

#[test]
fn touch_with_reference_maps_reference_read_and_target_write() {
    for args in [
        &["touch", "-r", "template.txt", "file.txt"][..],
        &["touch", "-rtemplate.txt", "file.txt"],
        &["touch", "--reference", "template.txt", "file.txt"],
        &["touch", "--reference=template.txt", "file.txt"],
    ] {
        let out = summarize_argv(&argv(args));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![
                Capability {
                    resource: Resource::Fs,
                    action: Action::Read,
                    scope: "template.txt".to_string(),
                },
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "file.txt".to_string(),
                }
            ]
        );
    }
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
