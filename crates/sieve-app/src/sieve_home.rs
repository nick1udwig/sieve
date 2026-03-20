use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const DEFAULT_GIT_USER_NAME: &str = "Sieve Runtime";
const DEFAULT_GIT_USER_EMAIL: &str = "sieve@localhost";
const GITIGNORE_MARKER_START: &str = "# --- sieve runtime ignores ---";
const GITIGNORE_MARKER_END: &str = "# --- /sieve runtime ignores ---";
const AGENTS_MARKER_START: &str = "<!-- sieve home description -->";
const AGENTS_MARKER_END: &str = "<!-- /sieve home description -->";
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

const MANAGED_GITIGNORE_BLOCK: &str = "\
# --- sieve runtime ignores ---
/artifacts/
/logs/
/media/
/lcm/
/state/
# --- /sieve runtime ignores ---
";

const MANAGED_AGENTS_BLOCK: &str = "\
<!-- sieve home description -->
# Sieve Home

This directory is the runtime home for the sieve system.
This git repository captures durable local configuration and operator-authored notes.
Runtime noise stays mostly untracked.

## Structure

- `AGENTS.md`: local description of this sieve home.
- `.gitignore`: runtime-noise ignore rules for this repo.
- `config/`: tracked local config and notes.
- `state/`: runtime databases, approvals, auth material, usually ignored.
- `logs/`: runtime event and provider logs, ignored.
- `artifacts/`: per-turn artifacts, ignored.
- `media/`: downloaded or uploaded media, ignored.
- `lcm/`: local memory databases, ignored.

## Notes

Keep secrets out of tracked files.
Prefer small, reviewable commits.
<!-- /sieve home description -->
";

pub(crate) fn ensure_sieve_home_repo(sieve_home: &Path) -> Result<(), String> {
    fs::create_dir_all(sieve_home)
        .map_err(|err| format!("create sieve home {} failed: {err}", sieve_home.display()))?;
    create_standard_dirs(sieve_home)?;
    ensure_git_repo(sieve_home)?;
    ensure_git_identity(sieve_home)?;
    let mut changed = false;
    changed |= ensure_managed_block(
        &sieve_home.join(".gitignore"),
        GITIGNORE_MARKER_START,
        GITIGNORE_MARKER_END,
        MANAGED_GITIGNORE_BLOCK,
    )?;
    changed |= ensure_managed_block(
        &sieve_home.join("AGENTS.md"),
        AGENTS_MARKER_START,
        AGENTS_MARKER_END,
        MANAGED_AGENTS_BLOCK,
    )?;
    if changed {
        commit_sieve_home_changes(sieve_home, "chore: initialize sieve home")?;
    }
    Ok(())
}

pub(crate) fn spawn_sieve_home_git_watcher(sieve_home: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            if let Err(err) = maybe_commit_sieve_home_changes(&sieve_home) {
                eprintln!(
                    "sieve home auto-commit failed for {}: {}",
                    sieve_home.display(),
                    err
                );
            }
        }
    })
}

pub(crate) fn maybe_commit_sieve_home_changes(sieve_home: &Path) -> Result<bool, String> {
    if !sieve_home.join(".git").exists() {
        return Ok(false);
    }
    if !git_status_has_changes(sieve_home)? {
        return Ok(false);
    }
    let commit_message = staged_commit_message(sieve_home)?;
    commit_sieve_home_changes(sieve_home, &commit_message)?;
    Ok(true)
}

fn create_standard_dirs(sieve_home: &Path) -> Result<(), String> {
    for dir in ["config", "state", "logs", "artifacts", "media", "lcm"] {
        fs::create_dir_all(sieve_home.join(dir)).map_err(|err| {
            format!(
                "create sieve home dir {} failed: {err}",
                sieve_home.join(dir).display()
            )
        })?;
    }
    Ok(())
}

fn ensure_git_repo(sieve_home: &Path) -> Result<(), String> {
    if sieve_home.join(".git").exists() {
        return Ok(());
    }
    run_git(sieve_home, ["init"])?;
    Ok(())
}

fn ensure_git_identity(sieve_home: &Path) -> Result<(), String> {
    if git_config_value(sieve_home, "user.name")?.is_none() {
        run_git(sieve_home, ["config", "user.name", DEFAULT_GIT_USER_NAME])?;
    }
    if git_config_value(sieve_home, "user.email")?.is_none() {
        run_git(sieve_home, ["config", "user.email", DEFAULT_GIT_USER_EMAIL])?;
    }
    Ok(())
}

fn git_config_value(sieve_home: &Path, key: &str) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(sieve_home)
        .output()
        .map_err(|err| format!("git config --get {key} failed: {err}"))?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(value))
        }
    } else {
        Ok(None)
    }
}

fn ensure_managed_block(
    path: &Path,
    marker_start: &str,
    marker_end: &str,
    block: &str,
) -> Result<bool, String> {
    let existing = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("read {} failed: {err}", path.display())),
    };
    if existing.contains(marker_start) && existing.contains(marker_end) {
        return Ok(false);
    }
    let mut updated = existing;
    if !updated.trim().is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.trim().is_empty() {
        updated.push('\n');
    }
    updated.push_str(block);
    fs::write(path, updated).map_err(|err| format!("write {} failed: {err}", path.display()))?;
    Ok(true)
}

fn git_status_has_changes(sieve_home: &Path) -> Result<bool, String> {
    let output = run_git_capture(sieve_home, ["status", "--short"])?;
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

fn staged_commit_message(sieve_home: &Path) -> Result<String, String> {
    let output = run_git_capture(sieve_home, ["status", "--short"])?;
    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let path = line[3..].trim();
        if path.is_empty() {
            continue;
        }
        paths.push(path.to_string());
    }
    if paths
        .iter()
        .any(|path| path == "AGENTS.md" || path == ".gitignore")
    {
        return Ok("chore: update sieve home metadata".to_string());
    }
    if paths.iter().any(|path| path.starts_with("config/")) {
        return Ok("chore: update sieve home config".to_string());
    }
    Ok("chore: update sieve home".to_string())
}

fn commit_sieve_home_changes(sieve_home: &Path, message: &str) -> Result<(), String> {
    run_git(sieve_home, ["add", "-A", "."])?;
    let diff = run_git_capture(sieve_home, ["diff", "--cached", "--name-only"])?;
    if String::from_utf8_lossy(&diff.stdout).trim().is_empty() {
        return Ok(());
    }
    run_git(sieve_home, ["commit", "-m", message, "--no-verify"])?;
    Ok(())
}

fn run_git<const N: usize>(sieve_home: &Path, args: [&str; N]) -> Result<(), String> {
    let output = run_git_capture(sieve_home, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format_git_failure(&args, &output))
    }
}

fn run_git_capture<const N: usize>(
    sieve_home: &Path,
    args: [&str; N],
) -> Result<std::process::Output, String> {
    Command::new(OsStr::new("git"))
        .args(args)
        .current_dir(sieve_home)
        .output()
        .map_err(|err| format!("git {:?} failed: {err}", args))
}

fn format_git_failure(args: &[&str], output: &std::process::Output) -> String {
    format!(
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}
