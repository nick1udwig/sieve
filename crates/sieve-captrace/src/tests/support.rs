use crate::fixture::{TOKEN_OUT_FILE, TOKEN_TMP_DIR};
use crate::generator::{TraceRequest, TraceRunner};
use crate::planner::{CaseGenerationRequest, CaseGenerator};
use crate::CapTraceError;
use async_trait::async_trait;
use sieve_types::{Action, Capability, QuarantineReport, Resource, RunId};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct StubTraceRunner;

#[async_trait]
impl TraceRunner for StubTraceRunner {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError> {
        let mut attempted_capabilities = Vec::new();
        if request.argv.first().map(String::as_str) == Some("mkdir") {
            if let Some(path) = request.argv.last() {
                attempted_capabilities.push(Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: path.clone(),
                });
            }
            attempted_capabilities.push(Capability {
                resource: Resource::Proc,
                action: Action::Exec,
                scope: "/usr/bin/mkdir".to_string(),
            });
        } else if request.argv.first().map(String::as_str) == Some("cp") {
            attempted_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: request.argv.get(1).cloned().unwrap_or_default(),
            });
            attempted_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: request.argv.get(2).cloned().unwrap_or_default(),
            });
            attempted_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/etc/ld.so.cache".to_string(),
            });
        }

        Ok(QuarantineReport {
            run_id: RunId(request.run_id),
            trace_path: "/tmp/stub-trace".to_string(),
            stdout_path: None,
            stderr_path: None,
            attempted_capabilities,
            exit_code: Some(0),
        })
    }
}

pub(super) struct CurlNetTraceRunner;

#[async_trait]
impl TraceRunner for CurlNetTraceRunner {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError> {
        let attempted_capabilities = if request.argv.first().map(String::as_str) == Some("curl") {
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "family=af_inet,address=example.com,port=443".to_string(),
            }]
        } else {
            Vec::new()
        };

        Ok(QuarantineReport {
            run_id: RunId(request.run_id),
            trace_path: "/tmp/stub-trace-curl".to_string(),
            stdout_path: None,
            stderr_path: None,
            attempted_capabilities,
            exit_code: Some(0),
        })
    }
}

pub(super) struct StubCaseGenerator {
    pub(super) cases: Vec<Vec<String>>,
}

#[async_trait]
impl CaseGenerator for StubCaseGenerator {
    async fn generate_cases(
        &self,
        _request: CaseGenerationRequest,
    ) -> Result<Vec<Vec<String>>, CapTraceError> {
        Ok(self.cases.clone())
    }
}

pub(super) fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.is_empty() {
                "''".to_string()
            } else if arg.chars().all(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.' | ':' | '=' | '@')
            }) {
                arg.clone()
            } else {
                let mut escaped = String::from("'");
                for ch in arg.chars() {
                    if ch == '\'' {
                        escaped.push_str("'\\''");
                    } else {
                        escaped.push(ch);
                    }
                }
                escaped.push('\'');
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn write_fake_help_script(path: &PathBuf) {
    let script = r#"#!/usr/bin/env bash
if [ "$1" = "--help" ]; then
cat <<'EOF'
bravesearch - Simple Brave Search CLI

Usage:
  bravesearch <command> [options]

Commands:
  search [query]      Run Brave web search
  news                Run Brave news search
  version             Print version
  help                Show this help
EOF
exit 0
fi

if [ "$1" = "search" ] && [ "$2" = "--help" ]; then
cat <<'EOF'
Usage:
  bravesearch search [flags] [query]

Flags:
  --q, --query string    Search query
  --count int            Number of results
EOF
exit 0
fi

if [ "$1" = "news" ] && [ "$2" = "--help" ]; then
cat <<'EOF'
Usage:
  bravesearch news [flags] [query]

Flags:
  --count int            Number of results
EOF
exit 0
fi

exit 1
"#
    .to_string();
    fs::write(path, script).expect("write fake command script");
    let mut permissions = fs::metadata(path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod fake command script");
}

pub(super) fn write_failing_script(path: &PathBuf) {
    let script = r#"#!/usr/bin/env bash
exit 1
"#
    .to_string();
    fs::write(path, script).expect("write failing command script");
    let mut permissions = fs::metadata(path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod failing command script");
}

pub(super) fn make_temp_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "sieve-captrace-test-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp test dir");
    fs::write(dir.join(TOKEN_OUT_FILE), b"out").expect("seed out file");
    fs::create_dir_all(dir.join(TOKEN_TMP_DIR)).expect("seed tmp dir");
    dir
}
