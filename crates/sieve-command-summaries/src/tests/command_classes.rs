use super::argv;
use crate::summarize_argv;
use sieve_types::CommandKnowledge;

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
