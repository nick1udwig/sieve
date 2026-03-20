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
    assert!(home.join(".gitignore").exists());
    assert!(home.join("AGENTS.md").exists());
    assert!(home.join("config").exists());

    let tracked = git(&home, &["ls-files"]);
    assert!(tracked.contains(".gitignore"));
    assert!(tracked.contains("AGENTS.md"));

    let log = git(&home, &["log", "--oneline", "--max-count=1"]);
    assert!(log.contains("initialize sieve home"));
}

#[test]
fn maybe_commit_sieve_home_changes_commits_config_but_skips_runtime_dirs() {
    let home = temp_home("commit");
    ensure_sieve_home_repo(&home).expect("initialize sieve home repo");

    fs::create_dir_all(home.join("config")).expect("create config dir");
    fs::create_dir_all(home.join("logs")).expect("create logs dir");
    fs::write(home.join("config/settings.toml"), "planner = \"gpt-5.4\"\n").expect("write config");
    fs::write(
        home.join("logs/runtime-events.jsonl"),
        "{\"event\":\"ignored\"}\n",
    )
    .expect("write log");

    let committed = maybe_commit_sieve_home_changes(&home).expect("commit home changes");
    assert!(committed);

    let tracked = git(&home, &["ls-files"]);
    assert!(tracked.contains("config/settings.toml"));
    assert!(!tracked.contains("logs/runtime-events.jsonl"));

    let changed = git(&home, &["show", "--stat", "--format=%s", "HEAD"]);
    assert!(changed.contains("update sieve home"));
    assert!(changed.contains("config/settings.toml"));

    let status = git(&home, &["status", "--short"]);
    assert!(status.trim().is_empty(), "unexpected git status: {status}");
}
