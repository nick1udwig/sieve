#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannerCommandDescriptor {
    pub command: &'static str,
    pub description: &'static str,
}

const PLANNER_COMMAND_CATALOG: &[PlannerCommandDescriptor] = &[
    PlannerCommandDescriptor {
        command: "bravesearch",
        description: "Search Brave index from CLI for discovery. Preferred pattern: `bravesearch search --query \"...\" --count N --output json` (`--output`, not `--format`). After discovery, fetch selected result URLs with `curl` for grounded facts.",
    },
    PlannerCommandDescriptor {
        command: "brave-search",
        description: "Alias for `bravesearch` with the same subcommands and flags (`--output`, not `--format`).",
    },
    PlannerCommandDescriptor {
        command: "curl",
        description: "Send HTTP requests directly (GET/POST/etc.) to fetch remote content or APIs. For webpage content, prefer `curl -sS \"https://markdown.new/<url>\"` over raw HTML for cleaner extraction. Avoid piping to uncataloged commands (for example `| head`) because policy may deny them.",
    },
    PlannerCommandDescriptor {
        command: "rm",
        description: "Remove files/directories; destructive, often policy-gated (for example recursive deletes).",
    },
    PlannerCommandDescriptor {
        command: "cp",
        description: "Copy files/directories to a destination path.",
    },
    PlannerCommandDescriptor {
        command: "mv",
        description: "Move or rename files/directories.",
    },
    PlannerCommandDescriptor {
        command: "mkdir",
        description: "Create directories (supports parent creation flags).",
    },
    PlannerCommandDescriptor {
        command: "touch",
        description: "Create files or update file timestamps.",
    },
    PlannerCommandDescriptor {
        command: "chmod",
        description: "Change file permission modes.",
    },
    PlannerCommandDescriptor {
        command: "chown",
        description: "Change file ownership.",
    },
    PlannerCommandDescriptor {
        command: "tee",
        description: "Write stdin to one or more files (optionally append).",
    },
    PlannerCommandDescriptor {
        command: "codex",
        description: "Run Codex non-interactively with `codex exec`. Read-only pattern: `codex exec --sandbox read-only --ephemeral \"...\"` (stdout only; optional `--search` and `--image PATH`). Workspace-write pattern: `codex exec --sandbox workspace-write -C <repo> [--add-dir <dir>] \"...\"`. `codex app-server` is intentionally unsupported here.",
    },
    PlannerCommandDescriptor {
        command: "sieve-lcm-cli",
        description: "Query persistent memory via CLI. Read path for planner: `sieve-lcm-cli query --lane both --query \"...\" --json` (trusted excerpts + untrusted refs). Resolve untrusted refs with `sieve-lcm-cli expand --ref <ref> --json` for qLLM/ref workflows.",
    },
    PlannerCommandDescriptor {
        command: "st",
        description: "Speech CLI for transcription and synthesis. STT pattern: `st stt <audio-file>` (prints transcript to stdout, optionally `-o <file>`). TTS pattern: `st tts <text-file> --format ogg --output <audio-file>` or `st tts --txt \"...\" --format ogg --output <audio-file>`.",
    },
];

pub fn planner_command_catalog() -> &'static [PlannerCommandDescriptor] {
    PLANNER_COMMAND_CATALOG
}
