use crate::st::PLANNER_CATALOG_DESCRIPTION;

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
        command: "codex",
        description: "Run Codex non-interactively `codex exec`. Read-only: `codex exec --sandbox read-only --ephemeral \"...\"` (stdout only; optional `--search` and `--image PATH`). Workspace-write: `codex exec --sandbox workspace-write -C <repo> [--add-dir <dir>] \"...\"`. `codex app-server` unsupported.",
    },
    PlannerCommandDescriptor {
        command: "gws",
        description: "Google Workspace CLI. Supported here: `gws schema <service.resource.method>` and API calls like `gws drive files list --params '{\"pageSize\":10}'` or `gws sheets spreadsheets values append --params '{...}' --json '{...}'`. Use `--dry-run` to inspect request shape locally with no network/file effects. Read-ish methods require Google API net connect; mutating methods or `--json`/`--upload` require Google API net write. `--upload PATH` also reads a local file and `--output PATH` writes a local file. Service `+helpers` and top-level `gws auth|workflow|modelarmor|mcp` are intentionally unsupported.",
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
