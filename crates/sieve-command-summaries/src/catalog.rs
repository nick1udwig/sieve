use crate::st::PLANNER_CATALOG_DESCRIPTION;
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannerCommandDescriptor {
    pub command: &'static str,
    pub description: &'static str,
}

const PLANNER_COMMAND_CATALOG: &[PlannerCommandDescriptor] = &[
    PlannerCommandDescriptor {
        command: "bravesearch",
        description: "Search Brave index from CLI for discovery. Preferred pattern: `bravesearch search --query \"...\" --count N --output json` (`--output`, not `--format`). After discovery, fetch selected result URLs with `curl` or `agent-browser` for grounded facts.",
    },
    PlannerCommandDescriptor {
        command: "brave-search",
        description: "Alias for `bravesearch` with the same subcommands and flags (`--output`, not `--format`).",
    },
    PlannerCommandDescriptor {
        command: "curl",
        description: "`curl`. For webpage content, prefer `curl -sS \"https://markdown.new/<url>\"` over raw HTML for cleaner extraction.",
    },
    PlannerCommandDescriptor {
        command: "agent-browser",
        description: "Browser automation. Must start with explicit-origin command e.g.: `agent-browser open <url>`, `agent-browser tab new <url>`, `agent-browser diff url <url1> <url2>`, `agent-browser connect <port|url>`, `agent-browser record start <file.webm> <url>`, `agent-browser cookies set <name> <value> --url <url>`. Followup commands include, i.e. `snapshot`, `click`, `fill`, `get`, `screenshot`, `pdf`, `download`, `upload`, `storage`. End session with `agent-browser close ...` Always use `--session`: starting command will set session name as passed, then followups and close refer to it.",
    },
    PlannerCommandDescriptor {
        command: "cp",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "trash",
        description: "Move files or directories to the desktop trash instead of permanently deleting them. Preferred form: `trash [--trash-dir <dir>] <path>...`.",
    },
    PlannerCommandDescriptor {
        command: "mv",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "mkdir",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "touch",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "chmod",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "chown",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "tee",
        description: "",
    },
    PlannerCommandDescriptor {
        command: "gws",
        description: "Google Workspace CLI. Supported here: `gws schema <service.resource.method>` and API calls like `gws drive files list --params '{\"pageSize\":10}'` or `gws sheets spreadsheets values append --params '{...}' --json '{...}'`. Schema targets are dotted, but real API calls are space-separated CLI tokens: `gws schema gmail.users.messages.list` maps to `gws gmail users messages list`. Never use dotted subcommands like `users.messages.list` in a real `gws` API call. Use `--dry-run` to inspect request shape locally with no network/file effects. Read-ish methods require Google API net connect; mutating methods or `--json`/`--upload` require Google API net write. `--upload PATH` also reads a local file and `--output PATH` writes a local file. Service `+helpers` and top-level `gws auth|workflow|modelarmor|mcp` are intentionally unsupported.",
    },
    PlannerCommandDescriptor {
        command: "sieve-lcm-cli",
        description: "Query persistent memory. Read path for planner: `sieve-lcm-cli query --lane both --query \"...\" --json` (trusted excerpts + untrusted refs). Resolve untrusted refs with `sieve-lcm-cli expand --ref <ref> --json` for qLLM/ref workflows.",
    },
    PlannerCommandDescriptor {
        command: "st",
        description: PLANNER_CATALOG_DESCRIPTION,
    },
];

pub fn planner_command_catalog() -> &'static [PlannerCommandDescriptor] {
    PLANNER_COMMAND_CATALOG
}

pub fn planner_available_command_catalog() -> Vec<PlannerCommandDescriptor> {
    planner_command_catalog()
        .iter()
        .copied()
        .filter(|descriptor| command_available(descriptor.command))
        .collect()
}

pub fn command_available(command: &str) -> bool {
    resolve_command(command).is_some()
}

pub fn resolve_command(command: &str) -> Option<PathBuf> {
    let candidate = Path::new(command);
    if candidate.components().count() > 1 {
        return executable_file(candidate).then(|| candidate.to_path_buf());
    }
    let path = env::var_os("PATH")?;
    resolve_command_in_path(command, &path)
}

fn resolve_command_in_path(command: &str, path: &std::ffi::OsStr) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|directory| directory.join(command))
        .find(|candidate| executable_file(candidate))
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    use std::fs;

    #[test]
    fn command_resolution_requires_an_executable_file() {
        let root =
            std::env::temp_dir().join(format!("sieve-command-catalog-{}", std::process::id()));
        fs::create_dir_all(&root).expect("temp dir");
        let command = root.join("example-tool");
        fs::write(&command, "#!/bin/sh\n").expect("write tool");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&command, fs::Permissions::from_mode(0o755))
                .expect("make executable");
        }
        let path = std::env::join_paths([root.as_path()]).expect("join path");
        assert_eq!(
            resolve_command_in_path("example-tool", &path),
            Some(command.clone())
        );
        assert!(resolve_command_in_path("missing-tool", &path).is_none());
        let _ = fs::remove_file(command);
        let _ = fs::remove_dir(root);
    }
}
