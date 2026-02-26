#![forbid(unsafe_code)]

use async_trait::async_trait;
use sieve_types::{
    Action, Capability, CommandSegment, QuarantineReport, QuarantineRunRequest, Resource,
};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use thiserror::Error;

const DEFAULT_TRACE_ROOT_REL: &str = ".sieve/logs/traces";

#[derive(Debug, Error)]
pub enum QuarantineRunError {
    #[error("sandbox execution failed: {0}")]
    Exec(String),
}

#[async_trait]
pub trait QuarantineRunner: Send + Sync {
    async fn run(
        &self,
        request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError>;
}

#[derive(Debug, Clone)]
pub struct BwrapQuarantineRunner {
    logs_root: PathBuf,
    bwrap_program: String,
    strace_program: String,
    shell_program: String,
}

impl Default for BwrapQuarantineRunner {
    fn default() -> Self {
        Self {
            logs_root: default_trace_root(),
            bwrap_program: "bwrap".to_string(),
            strace_program: "strace".to_string(),
            shell_program: "bash".to_string(),
        }
    }
}

impl BwrapQuarantineRunner {
    pub fn new(logs_root: PathBuf) -> Self {
        Self {
            logs_root,
            ..Self::default()
        }
    }

    pub fn with_programs(
        logs_root: PathBuf,
        bwrap_program: impl Into<String>,
        strace_program: impl Into<String>,
        shell_program: impl Into<String>,
    ) -> Self {
        Self {
            logs_root,
            bwrap_program: bwrap_program.into(),
            strace_program: strace_program.into(),
            shell_program: shell_program.into(),
        }
    }

    fn run_sync(
        &self,
        request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError> {
        if request.command_segments.is_empty() {
            return Err(QuarantineRunError::Exec(
                "quarantine request must include at least one command segment".to_string(),
            ));
        }

        let run_dir = self.logs_root.join(&request.run_id.0);
        fs::create_dir_all(&run_dir).map_err(io_err)?;

        let stdout_path = run_dir.join("stdout.log");
        let stderr_path = run_dir.join("stderr.log");
        let trace_base = run_dir.join("strace");

        let command_script = command_segments_to_script(&request.command_segments)
            .map_err(QuarantineRunError::Exec)?;
        let output = self.execute_quarantine(&request, &run_dir, &trace_base, &command_script)?;

        fs::write(&stdout_path, &output.stdout).map_err(io_err)?;
        fs::write(&stderr_path, &output.stderr).map_err(io_err)?;

        let trace_files = collect_trace_files(&run_dir)?;
        if trace_files.is_empty() {
            return Err(QuarantineRunError::Exec(format!(
                "quarantine produced no trace artifacts (exit_code={:?}, stderr={})",
                output.status.code(),
                truncate_bytes_for_error(&output.stderr, 240)
            )));
        }
        let attempted_capabilities = parse_trace_capabilities(&trace_files)?;

        Ok(QuarantineReport {
            run_id: request.run_id,
            trace_path: run_dir.to_string_lossy().to_string(),
            stdout_path: Some(stdout_path.to_string_lossy().to_string()),
            stderr_path: Some(stderr_path.to_string_lossy().to_string()),
            attempted_capabilities,
            exit_code: output.status.code(),
        })
    }

    fn execute_quarantine(
        &self,
        request: &QuarantineRunRequest,
        run_dir: &Path,
        trace_base: &Path,
        command_script: &str,
    ) -> Result<Output, QuarantineRunError> {
        let bwrap_args = vec![
            "--die-with-parent".to_string(),
            "--new-session".to_string(),
            "--unshare-net".to_string(),
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
            "--bind".to_string(),
            run_dir.to_string_lossy().to_string(),
            run_dir.to_string_lossy().to_string(),
            "--chdir".to_string(),
            request.cwd.clone(),
            "--".to_string(),
            self.strace_program.clone(),
            "-ff".to_string(),
            "-s".to_string(),
            "4096".to_string(),
            "-o".to_string(),
            trace_base.to_string_lossy().to_string(),
            self.shell_program.clone(),
            "-lc".to_string(),
            command_script.to_string(),
        ];

        Command::new(&self.bwrap_program)
            .args(bwrap_args)
            .output()
            .map_err(|err| {
                let detail = if err.kind() == io::ErrorKind::NotFound {
                    format!(
                        "required executable missing: {} ({err})",
                        self.bwrap_program
                    )
                } else {
                    err.to_string()
                };
                QuarantineRunError::Exec(detail)
            })
    }
}

#[async_trait]
impl QuarantineRunner for BwrapQuarantineRunner {
    async fn run(
        &self,
        request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError> {
        self.run_sync(request)
    }
}

fn command_segments_to_script(segments: &[CommandSegment]) -> Result<String, String> {
    let mut script_parts = Vec::with_capacity(segments.len() * 2);

    for (index, segment) in segments.iter().enumerate() {
        if segment.argv.is_empty() {
            return Err(format!("command segment at index {index} has empty argv"));
        }

        if index > 0 {
            let op = segment.operator_before.as_ref().ok_or_else(|| {
                format!("command segment at index {index} missing operator_before")
            })?;
            script_parts.push(operator_token(op).to_string());
        }

        let escaped = segment
            .argv
            .iter()
            .map(|arg| shell_escape_single_quoted(arg))
            .collect::<Vec<_>>()
            .join(" ");
        script_parts.push(escaped);
    }

    Ok(script_parts.join(" "))
}

fn operator_token(operator: &sieve_types::CompositionOperator) -> &'static str {
    match operator {
        sieve_types::CompositionOperator::Sequence => ";",
        sieve_types::CompositionOperator::And => "&&",
        sieve_types::CompositionOperator::Or => "||",
        sieve_types::CompositionOperator::Pipe => "|",
    }
}

fn shell_escape_single_quoted(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn default_trace_root() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(DEFAULT_TRACE_ROOT_REL);
    }
    PathBuf::from("/tmp").join(DEFAULT_TRACE_ROOT_REL)
}

fn collect_trace_files(run_dir: &Path) -> Result<Vec<PathBuf>, QuarantineRunError> {
    let mut files = Vec::new();

    let entries = fs::read_dir(run_dir).map_err(io_err)?;
    for entry in entries {
        let entry = entry.map_err(io_err)?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == "strace" || name.starts_with("strace.") {
            files.push(path);
        }
    }

    files.sort();
    Ok(files)
}

fn parse_trace_capabilities(
    trace_files: &[PathBuf],
) -> Result<Vec<Capability>, QuarantineRunError> {
    let mut set = BTreeSet::new();

    for path in trace_files {
        let content = fs::read_to_string(path).map_err(io_err)?;
        for line in content.lines() {
            if let Some(capability) = parse_trace_line(line) {
                set.insert((
                    resource_order(capability.resource),
                    action_order(capability.action),
                    capability.scope,
                ));
            }
        }
    }

    Ok(set
        .into_iter()
        .map(|(resource_key, action_key, scope)| Capability {
            resource: resource_from_order(resource_key),
            action: action_from_order(action_key),
            scope,
        })
        .collect())
}

fn parse_trace_line(line: &str) -> Option<Capability> {
    if line.contains("execve(") || line.contains("execveat(") {
        let scope = extract_first_quoted(line)?;
        return Some(Capability {
            resource: Resource::Proc,
            action: Action::Exec,
            scope,
        });
    }

    if is_open_family(line) {
        let scope = extract_first_quoted(line)?;
        let action = action_from_open_flags(line);
        return Some(Capability {
            resource: Resource::Fs,
            action,
            scope,
        });
    }

    if is_mutating_fs_call(line) {
        let scope = extract_first_quoted(line)?;
        return Some(Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope,
        });
    }

    if line.contains("connect(") {
        if line.contains("AF_UNIX") {
            let scope =
                extract_named_quoted(line, "sun_path=").or_else(|| extract_first_quoted(line))?;
            return Some(Capability {
                resource: Resource::Ipc,
                action: Action::Connect,
                scope,
            });
        }

        if line.contains("AF_INET") || line.contains("AF_INET6") {
            let address = extract_named_quoted(line, "inet_addr(")
                .or_else(|| extract_named_quoted(line, "inet_pton(AF_INET6,"))
                .or_else(|| extract_first_quoted(line))
                .unwrap_or_else(|| "unknown".to_string());
            let port = extract_port(line).unwrap_or(0);
            return Some(Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: format!("{address}:{port}"),
            });
        }
    }

    None
}

fn is_open_family(line: &str) -> bool {
    line.contains("open(")
        || line.contains("openat(")
        || line.contains("openat2(")
        || line.contains("creat(")
}

fn is_mutating_fs_call(line: &str) -> bool {
    [
        "unlink(",
        "unlinkat(",
        "rename(",
        "renameat(",
        "renameat2(",
        "mkdir(",
        "mkdirat(",
        "rmdir(",
        "chmod(",
        "fchmod(",
        "fchmodat(",
        "chown(",
        "fchown(",
        "lchown(",
        "truncate(",
        "ftruncate(",
        "utime(",
        "utimes(",
        "utimensat(",
        "link(",
        "linkat(",
        "symlink(",
        "symlinkat(",
        "mknod(",
        "mknodat(",
    ]
    .iter()
    .any(|needle| line.contains(needle))
}

fn action_from_open_flags(line: &str) -> Action {
    if line.contains("O_APPEND") {
        return Action::Append;
    }

    if line.contains("O_WRONLY")
        || line.contains("O_RDWR")
        || line.contains("O_CREAT")
        || line.contains("O_TRUNC")
    {
        return Action::Write;
    }

    Action::Read
}

fn extract_first_quoted(input: &str) -> Option<String> {
    let start = input.find('"')? + 1;
    let rest = &input[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_named_quoted(input: &str, marker: &str) -> Option<String> {
    let idx = input.find(marker)?;
    extract_first_quoted(&input[idx..])
}

fn extract_port(line: &str) -> Option<u16> {
    let marker = "htons(";
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(')')?;
    tail[..end].trim().parse::<u16>().ok()
}

fn resource_order(resource: Resource) -> u8 {
    match resource {
        Resource::Fs => 0,
        Resource::Net => 1,
        Resource::Proc => 2,
        Resource::Env => 3,
        Resource::Ipc => 4,
    }
}

fn action_order(action: Action) -> u8 {
    match action {
        Action::Read => 0,
        Action::Write => 1,
        Action::Append => 2,
        Action::Exec => 3,
        Action::Connect => 4,
    }
}

fn resource_from_order(key: u8) -> Resource {
    match key {
        0 => Resource::Fs,
        1 => Resource::Net,
        2 => Resource::Proc,
        3 => Resource::Env,
        4 => Resource::Ipc,
        _ => Resource::Proc,
    }
}

fn action_from_order(key: u8) -> Action {
    match key {
        0 => Action::Read,
        1 => Action::Write,
        2 => Action::Append,
        3 => Action::Exec,
        4 => Action::Connect,
        _ => Action::Exec,
    }
}

fn io_err(err: io::Error) -> QuarantineRunError {
    QuarantineRunError::Exec(err.to_string())
}

fn truncate_bytes_for_error(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::{CommandSegment, CompositionOperator, RunId};
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn command_script_rebuilds_composed_segments() {
        let script = command_segments_to_script(&[
            CommandSegment {
                argv: vec!["printf".to_string(), "hello".to_string()],
                operator_before: None,
            },
            CommandSegment {
                argv: vec!["grep".to_string(), "h".to_string()],
                operator_before: Some(CompositionOperator::Pipe),
            },
            CommandSegment {
                argv: vec!["echo".to_string(), "done".to_string()],
                operator_before: Some(CompositionOperator::And),
            },
        ])
        .expect("script should build");

        assert_eq!(script, "'printf' 'hello' | 'grep' 'h' && 'echo' 'done'");
    }

    #[test]
    fn parse_trace_capabilities_normalizes_and_deduplicates() {
        let run_dir = unique_temp_dir();
        fs::create_dir_all(&run_dir).expect("temp dir");

        fs::write(
            run_dir.join("strace.101"),
            "execve(\"/bin/ls\", [\"ls\"], 0x0) = 0\nopenat(AT_FDCWD, \"/tmp/out\", O_WRONLY|O_CREAT, 0600) = 3\nconnect(3, {sa_family=AF_INET, sin_port=htons(443), sin_addr=inet_addr(\"1.2.3.4\")}, 16) = -1\n",
        )
        .expect("write trace");

        fs::write(
            run_dir.join("strace.202"),
            "openat(AT_FDCWD, \"/tmp/out\", O_WRONLY|O_CREAT, 0600) = 4\nconnect(3, {sa_family=AF_UNIX, sun_path=\"/tmp/socket\"}, 110) = 0\n",
        )
        .expect("write trace");

        let traces = collect_trace_files(&run_dir).expect("collect traces");
        let caps = parse_trace_capabilities(&traces).expect("parse capabilities");

        assert_eq!(
            caps,
            vec![
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "/tmp/out".to_string(),
                },
                Capability {
                    resource: Resource::Net,
                    action: Action::Connect,
                    scope: "1.2.3.4:443".to_string(),
                },
                Capability {
                    resource: Resource::Proc,
                    action: Action::Exec,
                    scope: "/bin/ls".to_string(),
                },
                Capability {
                    resource: Resource::Ipc,
                    action: Action::Connect,
                    scope: "/tmp/socket".to_string(),
                },
            ]
        );

        fs::remove_dir_all(&run_dir).expect("cleanup");
    }

    #[test]
    fn report_paths_follow_run_directory_layout() {
        let root = unique_temp_dir();
        let runner = BwrapQuarantineRunner::new(root.clone());
        let request = QuarantineRunRequest {
            run_id: RunId("run-123".to_string()),
            cwd: "/".to_string(),
            command_segments: vec![CommandSegment {
                argv: vec!["echo".to_string(), "hi".to_string()],
                operator_before: None,
            }],
        };

        let run_dir = runner.logs_root.join(&request.run_id.0);
        assert_eq!(run_dir, root.join("run-123"));
    }

    #[test]
    fn run_sync_generates_report_and_artifacts_with_fake_bwrap() {
        let root = unique_temp_dir();
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");

        let fake_bwrap = bin_dir.join("fake-bwrap");
        fs::write(
            &fake_bwrap,
            "#!/usr/bin/env bash\nset -euo pipefail\ntrace_base=\"\"\nfor ((i=1;i<=${#};i++)); do\n  arg=\"${!i}\"\n  if [[ \"$arg\" == \"-o\" ]]; then\n    next=$((i+1))\n    trace_base=\"${!next}\"\n  fi\ndone\necho 'execve(\"/bin/echo\", [\"echo\"], 0x0) = 0' > \"${trace_base}.123\"\necho fake-stdout\necho fake-stderr >&2\n",
        )
        .expect("write fake bwrap");
        let mut perms = fs::metadata(&fake_bwrap).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_bwrap, perms).expect("chmod");

        let runner = BwrapQuarantineRunner::with_programs(
            root.join(".sieve/logs/traces"),
            fake_bwrap.to_string_lossy().to_string(),
            "strace",
            "bash",
        );

        let report = runner
            .run_sync(QuarantineRunRequest {
                run_id: RunId("run-fake".to_string()),
                cwd: "/".to_string(),
                command_segments: vec![CommandSegment {
                    argv: vec!["echo".to_string(), "hello".to_string()],
                    operator_before: None,
                }],
            })
            .expect("run");

        assert_eq!(report.run_id.0, "run-fake");
        assert_eq!(report.exit_code, Some(0));
        assert!(report.trace_path.ends_with("run-fake"));

        let stdout_path = report.stdout_path.expect("stdout path");
        let stderr_path = report.stderr_path.expect("stderr path");
        let stdout_content = fs::read_to_string(stdout_path).expect("stdout content");
        let stderr_content = fs::read_to_string(stderr_path).expect("stderr content");
        assert_eq!(stdout_content, "fake-stdout\n");
        assert_eq!(stderr_content, "fake-stderr\n");

        assert_eq!(
            report.attempted_capabilities,
            vec![Capability {
                resource: Resource::Proc,
                action: Action::Exec,
                scope: "/bin/echo".to_string(),
            }]
        );

        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn run_sync_returns_error_when_trace_artifacts_missing() {
        let root = unique_temp_dir();
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");

        let fake_bwrap = bin_dir.join("fake-bwrap-fail");
        fs::write(
            &fake_bwrap,
            "#!/usr/bin/env bash\necho missing-trace >&2\nexit 42\n",
        )
        .expect("write fake bwrap fail");
        let mut perms = fs::metadata(&fake_bwrap).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_bwrap, perms).expect("chmod");

        let runner = BwrapQuarantineRunner::with_programs(
            root.join(".sieve/logs/traces"),
            fake_bwrap.to_string_lossy().to_string(),
            "strace",
            "bash",
        );

        let err = runner
            .run_sync(QuarantineRunRequest {
                run_id: RunId("run-fail".to_string()),
                cwd: "/".to_string(),
                command_segments: vec![CommandSegment {
                    argv: vec!["echo".to_string(), "hello".to_string()],
                    operator_before: None,
                }],
            })
            .expect_err("expected failure");

        let msg = err.to_string();
        assert!(msg.contains("produced no trace artifacts"));
        assert!(msg.contains("exit_code=Some(42)"));
        assert!(msg.contains("missing-trace"));

        fs::remove_dir_all(&root).expect("cleanup");
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        env::temp_dir().join(format!("sieve-quarantine-test-{nanos}"))
    }
}
