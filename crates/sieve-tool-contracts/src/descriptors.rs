use crate::{
    TOOL_AUTOMATION, TOOL_BASH, TOOL_CODEX_EXEC, TOOL_CODEX_SESSION, TOOL_DECLASSIFY, TOOL_ENDORSE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub family: &'static str,
    pub description: &'static str,
    pub exposure: ToolExposure,
    pub when_to_use: &'static [&'static str],
    pub when_not_to_use: &'static [&'static str],
    pub usage_notes: &'static [&'static str],
    pub examples: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Always,
    RequiresAutomation,
    RequiresCodex,
    RequiresValueRefs,
}

impl ToolDescriptor {
    pub fn render_function_description(&self) -> String {
        let mut parts = vec![self.description.to_string()];
        if !self.when_to_use.is_empty() {
            parts.push(format!("Use when: {}.", self.when_to_use.join("; ")));
        }
        if !self.when_not_to_use.is_empty() {
            parts.push(format!("Avoid when: {}.", self.when_not_to_use.join("; ")));
        }
        if !self.usage_notes.is_empty() {
            parts.push(format!("Notes: {}.", self.usage_notes.join("; ")));
        }
        if !self.examples.is_empty() {
            parts.push(format!("Examples: {}.", self.examples.join("; ")));
        }
        parts.join(" ")
    }
}

const AUTOMATION_DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: TOOL_AUTOMATION,
    family: "automation",
    description: "Manage heartbeat and cron automation jobs.",
    exposure: ToolExposure::RequiresAutomation,
    when_to_use: &[
        "explicit reminders",
        "scheduling future work",
        "listing, pausing, resuming, or removing cron jobs",
    ],
    when_not_to_use: &[
        "retrieval, search, inspection, or triage questions",
        "current-state questions about external systems",
    ],
    usage_notes: &[
        "for cron_add use typed schedule objects only",
        "prefer target `main` unless the user explicitly wants isolated/background-only execution",
        "`at.timestamp` must be absolute RFC3339 or unix-ms text",
    ],
    examples: &[
        r#"{"action":"cron_add","target":"main","schedule":{"kind":"after","delay":"1m"},"prompt":"remind me to check deploys"}"#,
        r#"{"action":"cron_add","target":"main","schedule":{"kind":"at","timestamp":"2026-03-08T12:00:00Z"},"prompt":"send weekly summary"}"#,
        r#"{"action":"cron_list"}"#,
    ],
};

const BASH_DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: TOOL_BASH,
    family: "shell",
    description: "Run a cataloged shell command through runtime policy gates.",
    exposure: ToolExposure::Always,
    when_to_use: &["the task matches a command in BASH_COMMAND_CATALOG"],
    when_not_to_use: &[
        "the command is not in BASH_COMMAND_CATALOG",
        "a native tool already matches the task better",
    ],
    usage_notes: &[
        "use only commands listed in BASH_COMMAND_CATALOG",
        "do not guess uncataloged command shapes",
    ],
    examples: &[],
};

const CODEX_EXEC_DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: TOOL_CODEX_EXEC,
    family: "codex",
    description: "Run a one-off argv command inside a Codex sandbox.",
    exposure: ToolExposure::RequiresCodex,
    when_to_use: &["single command execution inside read-only or workspace-write Codex sandboxing"],
    when_not_to_use: &[
        "multi-step coding sessions",
        "tasks that need network access through Codex",
    ],
    usage_notes: &[
        "`command` must be an argv array, not shell text",
        "choose `read_only` for inspection and `workspace_write` for edits/tests/builds",
        "Codex sandboxes have no network in this system",
    ],
    examples: &[r#"{"command":["git","status"],"sandbox":"read_only","cwd":"/root/git/repo"}"#],
};

const CODEX_SESSION_DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: TOOL_CODEX_SESSION,
    family: "codex",
    description: "Run or resume a multi-step Codex agent session for coding and repo work.",
    exposure: ToolExposure::RequiresCodex,
    when_to_use: &[
        "coding, file manipulation, tests, or deeper repo work",
        "tasks that may need session persistence or continuation",
    ],
    when_not_to_use: &[
        "simple one-off command execution",
        "tasks that need network access through Codex",
    ],
    usage_notes: &[
        "omit `session_id` to start a new session",
        "set `session_id` only when resuming an existing saved Codex session",
        "choose `read_only` for review and `workspace_write` for edits/tests/builds",
        "do not shell out to `codex` through bash when this tool is available",
    ],
    examples: &[
        r#"{"instruction":"implement the next bounded increment","sandbox":"workspace_write","cwd":"/root/git/repo","writable_roots":["/root/git/repo"]}"#,
    ],
};

const ENDORSE_DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: TOOL_ENDORSE,
    family: "integrity",
    description: "Raise the integrity level of a labeled value_ref after approval.",
    exposure: ToolExposure::RequiresValueRefs,
    when_to_use: &["a trusted flow requires endorsing a specific value_ref"],
    when_not_to_use: &["ordinary task execution that does not involve value_ref integrity"],
    usage_notes: &["pass the exact value_ref and desired target_integrity"],
    examples: &[],
};

const DECLASSIFY_DESCRIPTOR: ToolDescriptor = ToolDescriptor {
    name: TOOL_DECLASSIFY,
    family: "integrity",
    description: "Allow one labeled value_ref to flow to one exact sink after approval.",
    exposure: ToolExposure::RequiresValueRefs,
    when_to_use: &["a trusted flow needs a specific value_ref released to one exact sink"],
    when_not_to_use: &["ordinary task execution that does not involve sink-gated value_refs"],
    usage_notes: &["sink must be one absolute URL sink key"],
    examples: &[],
};

static SUPPORTED_TOOL_DESCRIPTORS: [ToolDescriptor; 6] = [
    AUTOMATION_DESCRIPTOR,
    BASH_DESCRIPTOR,
    CODEX_EXEC_DESCRIPTOR,
    CODEX_SESSION_DESCRIPTOR,
    ENDORSE_DESCRIPTOR,
    DECLASSIFY_DESCRIPTOR,
];

pub fn supported_tool_descriptors() -> &'static [ToolDescriptor] {
    &SUPPORTED_TOOL_DESCRIPTORS
}

pub fn tool_descriptor(name: &str) -> Option<&'static ToolDescriptor> {
    SUPPORTED_TOOL_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.name == name)
}

pub fn is_native_planner_tool(name: &str) -> bool {
    tool_descriptor(name).is_some()
}

pub fn planner_exposed_tool_names(
    configured_tools: &[String],
    has_known_value_refs: bool,
    automation_available: bool,
    codex_available: bool,
) -> Vec<String> {
    configured_tools
        .iter()
        .filter(|tool_name| {
            let Some(descriptor) = tool_descriptor(tool_name) else {
                return false;
            };
            match descriptor.exposure {
                ToolExposure::Always => true,
                ToolExposure::RequiresAutomation => automation_available,
                ToolExposure::RequiresCodex => codex_available,
                ToolExposure::RequiresValueRefs => has_known_value_refs,
            }
        })
        .cloned()
        .collect()
}
