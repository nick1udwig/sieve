use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;

fn temp_home(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "sieve-home-{}-{}-{}",
        label,
        std::process::id(),
        now_ms()
    ));
    fs::create_dir_all(&root).expect("create temp home");
    root
}

fn git(home: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(home)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout utf8")
}

#[test]
fn ensure_sieve_home_repo_initializes_git_and_managed_docs() {
    let home = temp_home("init");

    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    assert!(home.join(".git").exists());
    assert!(home.join("AGENTS.md").exists());
    assert!(home.join("config").exists());

    let tracked = git(&home, &["ls-files"]);
    assert!(tracked.contains("AGENTS.md"));

    let log = git(&home, &["log", "--oneline", "--max-count=1"]);
    assert!(log.contains("initialize sieve home"));
}

#[test]
fn ensure_sieve_home_repo_removes_legacy_managed_gitignore_block() {
    let home = temp_home("gitignore-migrate");
    fs::write(
        home.join(".gitignore"),
        "# --- sieve runtime ignores ---\n/logs/\n/artifacts/\n# --- /sieve runtime ignores ---\n",
    )
    .expect("seed legacy gitignore");

    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    assert!(
        !home.join(".gitignore").exists(),
        "legacy managed gitignore should be removed"
    );
}

#[test]
fn ensure_sieve_home_repo_refreshes_managed_agents_block() {
    let home = temp_home("agents-refresh");
    fs::write(
        home.join("AGENTS.md"),
        "<!-- sieve home description -->\nold\n<!-- /sieve home description -->\n",
    )
    .expect("seed legacy agents");

    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    let body = fs::read_to_string(home.join("AGENTS.md")).expect("read agents");
    assert!(body.contains("periodic runtime history snapshots"));
    assert!(!body.contains("\nold\n"));
}

#[test]
fn ensure_sieve_home_repo_untracks_never_commit_state_files() {
    let home = temp_home("untrack-state");
    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    fs::write(home.join("state/auth.json"), "{\"token\":\"secret\"}\n").expect("write auth");
    fs::write(home.join("state/codex.db"), "sqlite-ish").expect("write db");
    git(&home, &["add", "state/auth.json", "state/codex.db"]);
    git(&home, &["commit", "-m", "seed tracked state"]);

    ensure_sieve_home_repo(&home).expect("reinitialize sieve home repo");
    commit_sieve_home_changes_for_bucket(&home, SieveHomeCommitBucket::Immediate)
        .expect("run immediate bucket");

    let tracked = git(&home, &["ls-files"]);
    assert!(!tracked.contains("state/auth.json"));
    assert!(!tracked.contains("state/codex.db"));
}

#[test]
fn immediate_commit_tracks_config_and_leaves_logs_for_periodic_commit() {
    let home = temp_home("immediate");
    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    fs::create_dir_all(home.join("config")).expect("create config dir");
    fs::create_dir_all(home.join("logs")).expect("create logs dir");
    fs::write(home.join("config/settings.toml"), "planner = \"gpt-5.4\"\n").expect("write config");
    fs::write(
        home.join("logs/runtime-events.jsonl"),
        "{\"event\":\"ignored\"}\n",
    )
    .expect("write log");

    let committed = commit_sieve_home_changes_for_bucket(&home, SieveHomeCommitBucket::Immediate)
        .expect("commit immediate home changes");
    assert!(committed);

    let tracked = git(&home, &["ls-files"]);
    assert!(tracked.contains("config/settings.toml"));
    assert!(!tracked.contains("logs/runtime-events.jsonl"));

    let changed = git(&home, &["show", "--stat", "--format=%s", "HEAD"]);
    assert!(changed.contains("update sieve home config"));
    assert!(changed.contains("config/settings.toml"));
    assert!(!changed.contains("logs/runtime-events.jsonl"));

    let status = git(&home, &["status", "--short"]);
    assert!(
        status.contains("logs/"),
        "expected logs to remain pending: {status}"
    );
}

#[test]
fn periodic_commit_tracks_runtime_history() {
    let home = temp_home("periodic");
    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    fs::create_dir_all(home.join("logs")).expect("create logs dir");
    fs::create_dir_all(home.join("artifacts/run-1")).expect("create artifacts dir");
    fs::write(
        home.join("logs/runtime-events.jsonl"),
        "{\"event\":\"runtime\"}\n",
    )
    .expect("write log");
    fs::write(home.join("artifacts/run-1/stdout.log"), "hello\n").expect("write artifact");

    let committed = commit_sieve_home_changes_for_bucket(&home, SieveHomeCommitBucket::Periodic)
        .expect("commit periodic home changes");
    assert!(committed);

    let tracked = git(&home, &["ls-files"]);
    assert!(tracked.contains("logs/runtime-events.jsonl"));
    assert!(tracked.contains("artifacts/run-1/stdout.log"));

    let changed = git(&home, &["show", "--stat", "--format=%s", "HEAD"]);
    assert!(changed.contains("checkpoint sieve runtime history"));

    let status = git(&home, &["status", "--short"]);
    assert!(status.trim().is_empty(), "unexpected git status: {status}");
}
