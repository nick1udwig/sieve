use super::argv;
use crate::summarize_argv;
use sieve_types::{Action, CommandKnowledge, Resource};

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
