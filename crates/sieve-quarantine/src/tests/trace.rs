use super::support::{fixture_path, unique_temp_dir};
use crate::{collect_trace_files, parse_trace_capabilities, parse_trace_line};
use sieve_types::{Action, Capability, Resource};
use std::fs;

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
