use super::support::unique_temp_dir;
use crate::{command_segments_to_script, BwrapQuarantineRunner, REPORT_FILE_NAME};
use sieve_types::{
    Action, Capability, CommandSegment, CompositionOperator, QuarantineRunRequest, Resource, RunId,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

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

    let report_json_path = PathBuf::from(&report.trace_path).join(REPORT_FILE_NAME);
    let report_json = fs::read_to_string(report_json_path).expect("report json");
    assert!(report_json.contains("\"run_id\": \"run-fake\""));
    assert!(report_json.contains("\"trace_files\": ["));
    assert!(report_json.contains("strace.123"));
    assert!(report_json.contains("\"attempted_capabilities\": ["));
    assert!(report_json.contains("\"resource\": \"proc\""));
    assert!(report_json.contains("\"action\": \"exec\""));

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
