use sieve_types::{Action, Capability, Resource};

pub(crate) fn parse_trace_line(line: &str) -> Option<Capability> {
    if line.contains("execve(") || line.contains("execveat(") {
        let scope = extract_first_quoted(line)?;
        return Some(Capability {
            resource: Resource::Proc,
            action: Action::Exec,
            scope,
        });
    }

    if let Some(capability) = parse_process_spawn_capability(line) {
        return Some(capability);
    }

    if let Some(capability) = parse_env_capability(line) {
        return Some(capability);
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

    if let Some(capability) = parse_connect_capability(line) {
        return Some(capability);
    }

    None
}

fn parse_process_spawn_capability(line: &str) -> Option<Capability> {
    let syscall = if line.contains("clone3(") {
        "clone3"
    } else if line.contains("clone(") {
        "clone"
    } else if line.contains("vfork(") {
        "vfork"
    } else if line.contains("fork(") {
        "fork"
    } else {
        return None;
    };

    let pid = extract_syscall_result_number(line);
    Some(Capability {
        resource: Resource::Proc,
        action: Action::Exec,
        scope: format!(
            "spawn.{syscall}:pid={}",
            pid.map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
    })
}

fn parse_env_capability(line: &str) -> Option<Capability> {
    if line.contains("getenv(") || line.contains("secure_getenv(") {
        let key = extract_first_quoted(line).unwrap_or_else(|| "unknown".to_string());
        return Some(Capability {
            resource: Resource::Env,
            action: Action::Read,
            scope: format!("key={}", normalize_scope_value(&key)),
        });
    }

    if line.contains("setenv(") || line.contains("putenv(") || line.contains("unsetenv(") {
        let key = extract_first_quoted(line).unwrap_or_else(|| "unknown".to_string());
        return Some(Capability {
            resource: Resource::Env,
            action: Action::Write,
            scope: format!("key={}", normalize_scope_value(&key)),
        });
    }

    if is_open_family(line) {
        let path = extract_first_quoted(line)?;
        if is_environment_path(&path) {
            return Some(Capability {
                resource: Resource::Env,
                action: action_from_open_flags(line),
                scope: normalize_env_scope(&path),
            });
        }
    }

    None
}

fn parse_connect_capability(line: &str) -> Option<Capability> {
    if !is_connect_related_call(line) {
        return None;
    }

    if line.contains("AF_UNIX") {
        let path = extract_named_quoted(line, "sun_path=")
            .or_else(|| extract_named_quoted(line, "path="))
            .unwrap_or_else(|| "unknown".to_string());
        return Some(Capability {
            resource: Resource::Ipc,
            action: Action::Connect,
            scope: format!("family=af_unix,path={}", normalize_scope_value(&path)),
        });
    }

    if line.contains("AF_INET6") {
        let address = extract_named_quoted(line, "inet_pton(AF_INET6,")
            .or_else(|| extract_named_quoted(line, "sin6_addr=inet_pton(AF_INET6,"))
            .or_else(|| extract_first_quoted(line))
            .unwrap_or_else(|| "unknown".to_string());
        let port = extract_port(line).unwrap_or(0);
        return Some(Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: format!(
                "family=af_inet6,address={},port={port}",
                normalize_scope_value(&address)
            ),
        });
    }

    if line.contains("AF_INET") {
        let address = extract_named_quoted(line, "inet_addr(")
            .or_else(|| extract_named_quoted(line, "sin_addr=inet_addr("))
            .or_else(|| extract_first_quoted(line))
            .unwrap_or_else(|| "unknown".to_string());
        let port = extract_port(line).unwrap_or(0);
        return Some(Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: format!(
                "family=af_inet,address={},port={port}",
                normalize_scope_value(&address)
            ),
        });
    }

    Some(Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: "family=unknown,address=unknown,port=0".to_string(),
    })
}

fn is_environment_path(path: &str) -> bool {
    path.ends_with("/environ")
}

fn normalize_env_scope(path: &str) -> String {
    let pid = path
        .strip_prefix("/proc/")
        .and_then(|rest| rest.strip_suffix("/environ"))
        .unwrap_or("unknown");
    format!("proc_environ:pid={}", normalize_scope_value(pid))
}

fn is_connect_related_call(line: &str) -> bool {
    [
        "connect(",
        "socket(",
        "sendto(",
        "sendmsg(",
        "recvfrom(",
        "recvmsg(",
        "bind(",
        "listen(",
        "accept(",
        "accept4(",
    ]
    .iter()
    .any(|needle| line.contains(needle))
}

fn normalize_scope_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "unknown".to_string();
    }

    trimmed.replace(',', "%2C")
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
    extract_numeric_wrapped(line, "htons(", ')')
        .or_else(|| extract_numeric_after(line, "sin_port="))
        .or_else(|| extract_numeric_after(line, "sin6_port="))
}

fn extract_numeric_wrapped(line: &str, marker: &str, terminator: char) -> Option<u16> {
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let end = tail.find(terminator)?;
    tail[..end].trim().parse::<u16>().ok()
}

fn extract_numeric_after(line: &str, marker: &str) -> Option<u16> {
    let start = line.find(marker)? + marker.len();
    let tail = &line[start..];
    let digits: String = tail.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u16>().ok()
}

fn extract_syscall_result_number(line: &str) -> Option<i64> {
    let marker = ") =";
    let start = line.find(marker)? + marker.len();
    let token = line[start..].trim_start().split_whitespace().next()?;
    let value = token.parse::<i64>().ok()?;
    if value > 0 {
        Some(value)
    } else {
        None
    }
}
