use crate::planner_command_catalog;
use crate::st::{
    PLANNER_CATALOG_DESCRIPTION, PLANNER_STT_EXAMPLE, PLANNER_TTS_FILE_EXAMPLE,
    PLANNER_TTS_INLINE_EXAMPLE,
};

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
    assert_eq!(entry.description, PLANNER_CATALOG_DESCRIPTION);
    assert!(entry.description.contains(PLANNER_STT_EXAMPLE));
    assert!(entry.description.contains(PLANNER_TTS_FILE_EXAMPLE));
    assert!(entry.description.contains(PLANNER_TTS_INLINE_EXAMPLE));
    assert!(entry.description.contains("--format opus"));
    assert!(!entry.description.contains("--format ogg"));
}
