use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const DEFAULT_GIT_USER_NAME: &str = "Sieve Runtime";
const DEFAULT_GIT_USER_EMAIL: &str = "sieve@localhost";
const AGENTS_MARKER_START: &str = "<!-- sieve home description -->";
const AGENTS_MARKER_END: &str = "<!-- /sieve home description -->";
const LEGACY_GITIGNORE_MARKER_START: &str = "# --- sieve runtime ignores ---";
const LEGACY_GITIGNORE_MARKER_END: &str = "# --- /sieve runtime ignores ---";
const WATCH_INTERVAL: Duration = Duration::from_secs(2);
const PERIODIC_RUNTIME_COMMIT_INTERVAL: Duration = Duration::from_secs(300);

const MANAGED_AGENTS_BLOCK: &str = "\
<!-- sieve home description -->
# Sieve Home

This directory is the runtime home for the sieve system.
This git repository captures durable local configuration, operator-authored notes, and periodic runtime history snapshots.
Runtime commits use two buckets.

## Structure

- `AGENTS.md`: local description of this sieve home.
- `config/`: tracked local config and notes, committed immediately.
- `state/`: runtime databases, approvals, auth material, not auto-committed.
- `logs/`: runtime event and provider logs, committed periodically.
- `artifacts/`: per-turn artifacts, committed periodically.
- `media/`: downloaded or uploaded media, committed periodically.
- `lcm/`: local memory databases, not auto-committed.

## Notes

Keep secrets out of tracked files.
Prefer small, reviewable commits.
<!-- /sieve home description -->
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SieveHomeCommitBucket {
    Immediate,
    Periodic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SieveHomePathClass {
    Immediate,
    Periodic,
    Never,
}

pub(crate) fn ensure_sieve_home_repo(sieve_home: &Path) -> Result<(), String> {
    fs::create_dir_all(sieve_home)
        .map_err(|err| format!("create sieve home {} failed: {err}", sieve_home.display()))?;
    create_standard_dirs(sieve_home)?;
    ensure_git_repo(sieve_home)?;
    ensure_git_identity(sieve_home)?;
    remove_legacy_managed_gitignore(sieve_home)?;
    let agents_changed = ensure_managed_block(
        &sieve_home.join("AGENTS.md"),
        AGENTS_MARKER_START,
        AGENTS_MARKER_END,
        MANAGED_AGENTS_BLOCK,
    )?;
    let untracked_paths = untrack_never_commit_paths(sieve_home)?;
    if agents_changed {
        run_git_vec(sieve_home, &["add", "-A", "--", "AGENTS.md"])?;
    }
    if agents_changed || !untracked_paths.is_empty() {
        commit_staged_changes(sieve_home, "chore: initialize sieve home")?;
    }
    Ok(())
}

pub(crate) fn spawn_sieve_home_git_watcher(sieve_home: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_periodic_commit_at = Instant::now();
        loop {
            tokio::time::sleep(WATCH_INTERVAL).await;
            if let Err(err) =
                commit_sieve_home_changes_for_bucket(&sieve_home, SieveHomeCommitBucket::Immediate)
            {
                eprintln!(
                    "sieve home immediate auto-commit failed for {}: {}",
                    sieve_home.display(),
                    err
                );
            }
            if last_periodic_commit_at.elapsed() < PERIODIC_RUNTIME_COMMIT_INTERVAL {
                continue;
            }
            if let Err(err) =
                commit_sieve_home_changes_for_bucket(&sieve_home, SieveHomeCommitBucket::Periodic)
            {
                eprintln!(
                    "sieve home periodic auto-commit failed for {}: {}",
                    sieve_home.display(),
                    err
                );
            }
            last_periodic_commit_at = Instant::now();
        }
    })
}

pub(crate) fn commit_sieve_home_changes_for_bucket(
    sieve_home: &Path,
    bucket: SieveHomeCommitBucket,
) -> Result<bool, String> {
    if !sieve_home.join(".git").exists() {
        return Ok(false);
    }
    let changed_paths = status_paths_for_bucket(sieve_home, bucket)?;
    if changed_paths.is_empty() {
        return Ok(false);
    }
    let commit_message = staged_commit_message(bucket, &changed_paths);
    let path_refs = changed_paths.iter().map(String::as_str).collect::<Vec<_>>();
    commit_paths(sieve_home, &path_refs, &commit_message)?;
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

fn remove_legacy_managed_gitignore(sieve_home: &Path) -> Result<(), String> {
    let path = sieve_home.join(".gitignore");
    let existing = match fs::read_to_string(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("read {} failed: {err}", path.display())),
    };
    let stripped = strip_managed_block(
        &existing,
        LEGACY_GITIGNORE_MARKER_START,
        LEGACY_GITIGNORE_MARKER_END,
    );
    if stripped == existing {
        return Ok(());
    }
    if stripped.trim().is_empty() {
        fs::remove_file(&path).map_err(|err| format!("remove {} failed: {err}", path.display()))?;
        return Ok(());
    }
    fs::write(&path, stripped).map_err(|err| format!("write {} failed: {err}", path.display()))
}

fn strip_managed_block(body: &str, marker_start: &str, marker_end: &str) -> String {
    let Some(start) = body.find(marker_start) else {
        return body.to_string();
    };
    let Some(end_marker_offset) = body[start..].find(marker_end) else {
        return body.to_string();
    };
    let end = start + end_marker_offset + marker_end.len();
    let mut updated = String::new();
    updated.push_str(&body[..start]);
    let suffix = body[end..].trim_start_matches('\n');
    if !updated.trim_end().is_empty() && !suffix.is_empty() {
        updated.push('\n');
    }
    updated.push_str(suffix);
    updated.trim().to_string() + if updated.trim().is_empty() { "" } else { "\n" }
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
    let desired = replace_or_append_managed_block(&existing, marker_start, marker_end, block);
    if desired == existing {
        return Ok(false);
    }
    fs::write(path, desired).map_err(|err| format!("write {} failed: {err}", path.display()))?;
    Ok(true)
}

fn replace_or_append_managed_block(
    existing: &str,
    marker_start: &str,
    marker_end: &str,
    block: &str,
) -> String {
    if existing.contains(marker_start) && existing.contains(marker_end) {
        return replace_managed_block(existing, marker_start, marker_end, block);
    }
    let mut updated = existing.to_string();
    if !updated.trim().is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !updated.trim().is_empty() {
        updated.push('\n');
    }
    updated.push_str(block);
    updated
}

fn replace_managed_block(
    existing: &str,
    marker_start: &str,
    marker_end: &str,
    block: &str,
) -> String {
    let Some(start) = existing.find(marker_start) else {
        return existing.to_string();
    };
    let Some(end_marker_offset) = existing[start..].find(marker_end) else {
        return existing.to_string();
    };
    let end = start + end_marker_offset + marker_end.len();
    let mut updated = String::new();
    updated.push_str(&existing[..start]);
    if !updated.trim_end().is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(block);
    let suffix = existing[end..].trim_start_matches('\n');
    if !suffix.is_empty() {
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(suffix);
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
    }
    updated
}

fn status_paths_for_bucket(
    sieve_home: &Path,
    bucket: SieveHomeCommitBucket,
) -> Result<Vec<String>, String> {
    let output = run_git_capture(sieve_home, ["status", "--short"])?;
    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        for candidate in split_status_paths(line[3..].trim()) {
            if !matches_bucket(classify_path(&candidate), bucket) {
                continue;
            }
            if !paths.iter().any(|existing| existing == &candidate) {
                paths.push(candidate);
            }
        }
    }
    Ok(paths)
}

fn split_status_paths(raw: &str) -> Vec<String> {
    if raw.contains(" -> ") {
        raw.split(" -> ")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    } else if raw.is_empty() {
        Vec::new()
    } else {
        vec![raw.to_string()]
    }
}

fn classify_path(path: &str) -> SieveHomePathClass {
    let normalized = path.trim_start_matches("./");
    if normalized == "state/auth.json"
        || normalized == "state/approval-allowances.json"
        || normalized.starts_with("lcm/")
        || normalized.ends_with(".db")
    {
        return SieveHomePathClass::Never;
    }
    if normalized.starts_with("logs/")
        || normalized.starts_with("artifacts/")
        || normalized.starts_with("media/")
    {
        return SieveHomePathClass::Periodic;
    }
    SieveHomePathClass::Immediate
}

fn legacy_tracked_never_paths(sieve_home: &Path) -> Result<Vec<String>, String> {
    let output = run_git_capture(sieve_home, ["ls-files"])?;
    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if classify_path(line) == SieveHomePathClass::Never {
            paths.push(line.to_string());
        }
    }
    Ok(paths)
}

fn untrack_never_commit_paths(sieve_home: &Path) -> Result<Vec<String>, String> {
    let tracked = legacy_tracked_never_paths(sieve_home)?;
    if tracked.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["rm", "--cached", "-r", "--ignore-unmatch", "--quiet"];
    args.extend(tracked.iter().map(String::as_str));
    run_git_vec(sieve_home, &args)?;
    Ok(tracked)
}

fn matches_bucket(class: SieveHomePathClass, bucket: SieveHomeCommitBucket) -> bool {
    match (class, bucket) {
        (SieveHomePathClass::Immediate, SieveHomeCommitBucket::Immediate) => true,
        (SieveHomePathClass::Periodic, SieveHomeCommitBucket::Periodic) => true,
        _ => false,
    }
}

fn staged_commit_message(bucket: SieveHomeCommitBucket, paths: &[String]) -> String {
    match bucket {
        SieveHomeCommitBucket::Periodic => "chore: checkpoint sieve runtime history".to_string(),
        SieveHomeCommitBucket::Immediate => {
            if paths.iter().any(|path| path == "AGENTS.md") {
                return "chore: update sieve home metadata".to_string();
            }
            if paths.iter().any(|path| path.starts_with("config/")) {
                return "chore: update sieve home config".to_string();
            }
            "chore: update sieve home".to_string()
        }
    }
}

fn commit_paths(sieve_home: &Path, paths: &[&str], message: &str) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut add_args = vec!["add", "-A", "--"];
    add_args.extend(paths.iter().copied());
    run_git_vec(sieve_home, &add_args)?;

    let mut diff_args = vec!["diff", "--cached", "--name-only", "--"];
    diff_args.extend(paths.iter().copied());
    let diff = run_git_vec_capture(sieve_home, &diff_args)?;
    if String::from_utf8_lossy(&diff.stdout).trim().is_empty() {
        return Ok(());
    }
    commit_staged_changes(sieve_home, message)
}

fn commit_staged_changes(sieve_home: &Path, message: &str) -> Result<(), String> {
    let diff = run_git_capture(sieve_home, ["diff", "--cached", "--name-only"])?;
    if String::from_utf8_lossy(&diff.stdout).trim().is_empty() {
        return Ok(());
    }
    run_git(sieve_home, ["commit", "-m", message, "--no-verify"])
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
    run_git_vec_capture(sieve_home, &args)
}

fn run_git_vec(sieve_home: &Path, args: &[&str]) -> Result<(), String> {
    let output = run_git_vec_capture(sieve_home, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format_git_failure(args, &output))
    }
}

fn run_git_vec_capture(sieve_home: &Path, args: &[&str]) -> Result<std::process::Output, String> {
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
