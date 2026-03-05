use super::*;
use sieve_types::{Action, Capability, CommandSegment, CompositionOperator, Resource, RunId};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
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
                scope: "family=af_inet,address=1.2.3.4,port=443".to_string(),
            },
            Capability {
                resource: Resource::Proc,
                action: Action::Exec,
                scope: "/bin/ls".to_string(),
            },
            Capability {
                resource: Resource::Ipc,
                action: Action::Connect,
                scope: "family=af_unix,path=/tmp/socket".to_string(),
            },
        ]
    );

    fs::remove_dir_all(&run_dir).expect("cleanup");
}

#[test]
fn parse_trace_line_normalizes_connect_families_and_unknowns() {
    let ipv6 = parse_trace_line(
        "connect(3, {sa_family=AF_INET6, sin6_port=htons(443), sin6_addr=inet_pton(AF_INET6, \"2001:db8::1\")}, 28) = 0",
    )
    .expect("ipv6 capability");
    assert_eq!(
        ipv6,
        Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "family=af_inet6,address=2001:db8::1,port=443".to_string(),
        }
    );

    let unix = parse_trace_line("socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 3")
        .expect("unix capability");
    assert_eq!(
        unix,
        Capability {
            resource: Resource::Ipc,
            action: Action::Connect,
            scope: "family=af_unix,path=unknown".to_string(),
        }
    );

    let sendto = parse_trace_line(
        "sendto(3, \"x\", 1, 0, {sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr(\"8.8.8.8\")}, 16) = 1",
    )
    .expect("sendto capability");
    assert_eq!(
        sendto,
        Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "family=af_inet,address=8.8.8.8,port=53".to_string(),
        }
    );

    let unknown = parse_trace_line("connect(3, {sa_family=AF_BLUETOOTH}, 16) = -1")
        .expect("unknown fallback capability");
    assert_eq!(
        unknown,
        Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "family=unknown,address=unknown,port=0".to_string(),
        }
    );
}

#[test]
fn parse_trace_line_infers_process_spawn_and_env_access() {
    let clone =
        parse_trace_line("clone(child_stack=NULL, flags=CLONE_CHILD_CLEARTID|SIGCHLD) = 4321")
            .expect("clone capability");
    assert_eq!(
        clone,
        Capability {
            resource: Resource::Proc,
            action: Action::Exec,
            scope: "spawn.clone:pid=4321".to_string(),
        }
    );

    let vfork = parse_trace_line("vfork() = 0").expect("vfork capability");
    assert_eq!(
        vfork,
        Capability {
            resource: Resource::Proc,
            action: Action::Exec,
            scope: "spawn.vfork:pid=unknown".to_string(),
        }
    );

    let env_read =
        parse_trace_line("openat(AT_FDCWD, \"/proc/self/environ\", O_RDONLY|O_CLOEXEC) = 3")
            .expect("env read capability");
    assert_eq!(
        env_read,
        Capability {
            resource: Resource::Env,
            action: Action::Read,
            scope: "proc_environ:pid=self".to_string(),
        }
    );

    let env_write =
        parse_trace_line("setenv(\"TOKEN\", \"secret\", 1) = 0").expect("env write capability");
    assert_eq!(
        env_write,
        Capability {
            resource: Resource::Env,
            action: Action::Write,
            scope: "key=TOKEN".to_string(),
        }
    );
}

#[test]
fn fixture_connect_trace_normalizes_endpoints() {
    let fixture = fixture_path("connect_trace.log");
    let caps = parse_trace_capabilities(&[fixture]).expect("parse fixture");

    assert!(caps.contains(&Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: "family=af_inet,address=1.2.3.4,port=443".to_string(),
    }));
    assert!(caps.contains(&Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: "family=af_inet6,address=2001:db8::1,port=53".to_string(),
    }));
    assert!(caps.contains(&Capability {
        resource: Resource::Ipc,
        action: Action::Connect,
        scope: "family=af_unix,path=/tmp/socket".to_string(),
    }));
    assert!(caps.contains(&Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: "family=unknown,address=unknown,port=0".to_string(),
    }));
}

#[test]
fn fixture_process_env_trace_infers_process_and_env_capabilities() {
    let fixture = fixture_path("process_env_trace.log");
    let caps = parse_trace_capabilities(&[fixture]).expect("parse fixture");

    assert!(caps.contains(&Capability {
        resource: Resource::Proc,
        action: Action::Exec,
        scope: "spawn.clone:pid=3210".to_string(),
    }));
    assert!(caps.contains(&Capability {
        resource: Resource::Env,
        action: Action::Read,
        scope: "proc_environ:pid=999".to_string(),
    }));
    assert!(caps.contains(&Capability {
        resource: Resource::Env,
        action: Action::Write,
        scope: "key=API_TOKEN".to_string(),
    }));
}

#[test]
fn report_paths_follow_run_directory_layout() {
    let root = unique_temp_dir();
    let runner = BwrapQuarantineRunner::new(root.clone());
    let request = sieve_types::QuarantineRunRequest {
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
        .run_sync(sieve_types::QuarantineRunRequest {
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
        .run_sync(sieve_types::QuarantineRunRequest {
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

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}
