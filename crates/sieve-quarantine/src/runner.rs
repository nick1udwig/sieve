use crate::report::{io_err, truncate_bytes_for_error, write_report_json};
use crate::trace::{collect_trace_files, parse_trace_capabilities};
use crate::QuarantineRunError;
use async_trait::async_trait;
use sieve_types::{QuarantineReport, QuarantineRunRequest};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DEFAULT_SIEVE_DIR_NAME: &str = ".sieve";

#[async_trait]
pub trait QuarantineRunner: Send + Sync {
    async fn run(
        &self,
        request: QuarantineRunRequest,
    ) -> Result<QuarantineReport, QuarantineRunError>;
}

#[derive(Debug, Clone)]
pub struct BwrapQuarantineRunner {
    pub(crate) logs_root: PathBuf,
    bwrap_program: String,
    strace_program: String,
    shell_program: String,
    network_mode: QuarantineNetworkMode,
    writable_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineNetworkMode {
    Isolated,
    LocalOnly,
    Full,
}

impl Default for BwrapQuarantineRunner {
    fn default() -> Self {
        Self {
            logs_root: default_trace_root(),
            bwrap_program: "bwrap".to_string(),
            strace_program: "strace".to_string(),
            shell_program: "bash".to_string(),
            network_mode: QuarantineNetworkMode::Isolated,
            writable_paths: Vec::new(),
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
            network_mode: QuarantineNetworkMode::Isolated,
            writable_paths: Vec::new(),
        }
    }

    pub fn with_sandbox(
        logs_root: PathBuf,
        network_mode: QuarantineNetworkMode,
        writable_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            logs_root,
            network_mode,
            writable_paths,
            ..Self::default()
        }
    }

    pub(crate) fn run_sync(
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
        let report = QuarantineReport {
            run_id: request.run_id,
            trace_path: run_dir.to_string_lossy().to_string(),
            stdout_path: Some(stdout_path.to_string_lossy().to_string()),
            stderr_path: Some(stderr_path.to_string_lossy().to_string()),
            attempted_capabilities,
            exit_code: output.status.code(),
        };
        write_report_json(&run_dir, &trace_files, &report)?;

        Ok(report)
    }

    fn execute_quarantine(
        &self,
        request: &QuarantineRunRequest,
        run_dir: &Path,
        trace_base: &Path,
        command_script: &str,
    ) -> Result<Output, QuarantineRunError> {
        let mut bwrap_args = vec!["--die-with-parent".to_string(), "--new-session".to_string()];
        if self.network_mode != QuarantineNetworkMode::Full {
            bwrap_args.push("--unshare-net".to_string());
        }
        bwrap_args.extend([
            "--ro-bind".to_string(),
            "/".to_string(),
            "/".to_string(),
            "--dev".to_string(),
            "/dev".to_string(),
            "--proc".to_string(),
            "/proc".to_string(),
            "--tmpfs".to_string(),
            "/tmp".to_string(),
        ]);
        for writable_path in &self.writable_paths {
            if !writable_path.exists() {
                return Err(QuarantineRunError::Exec(format!(
                    "writable path does not exist: {}",
                    writable_path.display()
                )));
            }
            bwrap_args.extend([
                "--bind".to_string(),
                writable_path.to_string_lossy().to_string(),
                writable_path.to_string_lossy().to_string(),
            ]);
        }
        bwrap_args.extend([
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
        ]);

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

pub(crate) fn command_segments_to_script(
    segments: &[sieve_types::CommandSegment],
) -> Result<String, String> {
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
    if let Ok(sieve_home) = env::var("SIEVE_HOME") {
        let sieve_home = sieve_home.trim();
        if !sieve_home.is_empty() {
            return PathBuf::from(sieve_home).join("logs/traces");
        }
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(DEFAULT_SIEVE_DIR_NAME)
            .join("logs/traces");
    }
    PathBuf::from("/tmp")
        .join(DEFAULT_SIEVE_DIR_NAME)
        .join("logs/traces")
}
