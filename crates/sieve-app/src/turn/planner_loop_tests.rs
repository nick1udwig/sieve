use super::*;
use sieve_types::CodexSandboxMode;

fn sample_session() -> PlannerCodexSession {
    PlannerCodexSession {
        session_id: "codex-session-1".to_string(),
        session_name: "you-are-starting".to_string(),
        cwd: "/root/git/modex".to_string(),
        sandbox: CodexSandboxMode::WorkspaceWrite,
        updated_at_utc: "2026-03-10T17:36:42Z".to_string(),
        status: "completed".to_string(),
        task_summary: "build modex".to_string(),
        last_result_summary: Some("a passing npm run build".to_string()),
    }
}

#[test]
fn codex_status_query_detection_matches_recent_phrase() {
    assert!(looks_like_codex_status_query(
        "how is you-are-starting doing?"
    ));
    assert!(looks_like_codex_status_query(
        "is you-are-starting done or is the work ongoing?"
    ));
    assert!(!looks_like_codex_status_query("resume you-are-starting"));
}

#[test]
fn find_referenced_codex_session_matches_session_name() {
    let sessions = vec![sample_session()];
    let matched = find_referenced_codex_session("how is you-are-starting doing?", &sessions)
        .expect("match codex session");
    assert_eq!(matched.session_id, "codex-session-1");
}

#[test]
fn find_referenced_codex_session_matches_cwd_basename() {
    let sessions = vec![sample_session()];
    let matched =
        find_referenced_codex_session("how is modex work going?", &sessions).expect("match");
    assert_eq!(matched.session_id, "codex-session-1");
}

#[test]
fn format_codex_session_status_reply_uses_saved_metadata() {
    let message = format_codex_session_status_reply(&sample_session());
    assert!(message.contains("It’s done, not ongoing."));
    assert!(message.contains("`you-are-starting`"));
    assert!(message.contains("`2026-03-10T17:36:42Z`"));
    assert!(message.contains("a passing npm run build"));
}
