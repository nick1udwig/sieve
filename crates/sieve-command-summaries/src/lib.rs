#![forbid(unsafe_code)]

use codex_shell_command::command_safety::is_dangerous_command::command_might_be_dangerous;
use codex_shell_command::command_safety::is_safe_command::is_known_safe_command;
use sieve_types::{Action, Capability, Resource, SinkCheck, SinkKey, ValueRef};
use sieve_types::{CommandKnowledge, CommandSummary};
use url::{Host, Url};

#[path = "brave-search.rs"]
mod brave_search;
mod codex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryOutcome {
    pub knowledge: CommandKnowledge,
    pub summary: Option<CommandSummary>,
    pub reason: Option<String>,
}

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
];

pub fn planner_command_catalog() -> &'static [PlannerCommandDescriptor] {
    PLANNER_COMMAND_CATALOG
}

pub trait CommandSummarizer: Send + Sync {
    fn summarize(&self, argv: &[String]) -> SummaryOutcome;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultCommandSummarizer;

impl CommandSummarizer for DefaultCommandSummarizer {
    fn summarize(&self, argv: &[String]) -> SummaryOutcome {
        summarize_argv(argv)
    }
}

fn summarize_argv(argv: &[String]) -> SummaryOutcome {
    if argv.is_empty() {
        return unknown_outcome("empty argv");
    }

    if let Some(outcome) = summarize_rm(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_cp(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_mv(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_mkdir(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_touch(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_chmod(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_chown(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_tee(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_curl(argv) {
        return outcome;
    }

    if let Some(outcome) = codex::summarize_codex_exec(argv) {
        return outcome;
    }

    if let Some(outcome) = summarize_sieve_lcm_cli(argv) {
        return outcome;
    }

    if let Some(outcome) = brave_search::summarize_brave_search(argv) {
        return outcome;
    }

    if is_known_safe_command(argv) {
        return known_outcome(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        });
    }

    if command_might_be_dangerous(argv) {
        return unknown_outcome("dangerous command class lacks explicit summary");
    }

    unknown_outcome("unknown command")
}

fn summarize_rm(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_rm_command(inner) {
        return None;
    }

    let mut recursive = false;
    let mut force = false;
    let mut saw_end_of_flags = false;
    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();

    for arg in inner.iter().skip(1) {
        if saw_end_of_flags {
            targets.push(arg.clone());
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            continue;
        }
        if arg.starts_with('-') {
            match arg.as_str() {
                "-r" | "-R" | "--recursive" => recursive = true,
                "-f" | "--force" => force = true,
                "-rf" | "-fr" => {
                    recursive = true;
                    force = true;
                }
                _ => unsupported_flags.push(arg.clone()),
            }
            continue;
        }
        targets.push(arg.clone());
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported rm flags",
            unsupported_flags,
        ));
    }

    if !(recursive && force) {
        return None;
    }

    if targets.is_empty() {
        targets.push("*".to_string());
    }

    Some(known_outcome(CommandSummary {
        required_capabilities: targets
            .into_iter()
            .map(|target| Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: target,
            })
            .collect(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    }))
}

fn summarize_cp(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "cp") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        inner,
        &['a', 'f', 'i', 'n', 'p', 'R', 'r', 'u', 'v'],
        &[
            "--archive",
            "--force",
            "--interactive",
            "--no-clobber",
            "--recursive",
            "--update",
            "--verbose",
            "--preserve",
        ],
        &["--preserve="],
    );
    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported cp flags",
            unsupported_flags,
        ));
    }
    if positionals.len() < 2 {
        return Some(unknown_outcome("cp missing destination"));
    }

    let destination = positionals.last().cloned().expect("checked above");
    Some(known_fs_outcome(vec![destination], Action::Write))
}

fn summarize_mv(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "mv") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        inner,
        &['f', 'i', 'n', 'u', 'v', 'T'],
        &[
            "--force",
            "--interactive",
            "--no-clobber",
            "--update",
            "--verbose",
            "--no-target-directory",
        ],
        &[],
    );
    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported mv flags",
            unsupported_flags,
        ));
    }
    if positionals.len() < 2 {
        return Some(unknown_outcome("mv missing destination"));
    }

    let mut scopes = positionals[..positionals.len() - 1].to_vec();
    scopes.push(positionals.last().cloned().expect("checked above"));
    Some(known_fs_outcome(scopes, Action::Write))
}

fn summarize_sieve_lcm_cli(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "sieve-lcm-cli") {
        return None;
    }

    let Some(subcommand) = inner.get(1).map(String::as_str) else {
        return Some(unknown_outcome("sieve-lcm-cli missing subcommand"));
    };

    match subcommand {
        "query" | "expand" => Some(known_outcome(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        })),
        "ingest" => {
            let db_path = flag_value(inner, "--db").unwrap_or_else(|| "~/.sieve/lcm".to_string());
            Some(known_fs_outcome(vec![db_path], Action::Write))
        }
        _ => Some(unknown_outcome("unknown sieve-lcm-cli command")),
    }
}

fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    argv.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
}

fn summarize_mkdir(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "mkdir") {
        return None;
    }

    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;
    let mut i = 1usize;

    while i < inner.len() {
        let arg = &inner[i];
        if saw_end_of_flags {
            targets.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            targets.push(arg.clone());
            i += 1;
            continue;
        }

        if arg == "-m" || arg == "--mode" {
            if i + 1 >= inner.len() {
                return Some(unknown_outcome("mkdir mode flag missing value"));
            }
            i += 2;
            continue;
        }
        if arg.starts_with("-m") && arg.len() > 2 {
            i += 1;
            continue;
        }
        if arg.starts_with("--mode=") {
            if arg == "--mode=" {
                return Some(unknown_outcome("mkdir mode flag missing value"));
            }
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            if matches!(arg.as_str(), "--parents" | "--verbose") {
                i += 1;
                continue;
            }
            unsupported_flags.push(arg.clone());
            i += 1;
            continue;
        }
        if is_short_flag_cluster(arg, &['p', 'v']) {
            i += 1;
            continue;
        }
        unsupported_flags.push(arg.clone());
        i += 1;
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported mkdir flags",
            unsupported_flags,
        ));
    }
    if targets.is_empty() {
        return Some(unknown_outcome("mkdir missing path"));
    }
    Some(known_fs_outcome(targets, Action::Write))
}

fn summarize_touch(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "touch") {
        return None;
    }

    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;
    let mut i = 1usize;

    while i < inner.len() {
        let arg = &inner[i];
        if saw_end_of_flags {
            targets.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            targets.push(arg.clone());
            i += 1;
            continue;
        }

        if matches!(
            arg.as_str(),
            "-d" | "-r" | "-t" | "--date" | "--reference" | "--time"
        ) {
            if i + 1 >= inner.len() {
                return Some(unknown_outcome("touch time/reference flag missing value"));
            }
            i += 2;
            continue;
        }
        if matches!(arg.as_str(), "--date=" | "--reference=" | "--time=") {
            return Some(unknown_outcome("touch time/reference flag missing value"));
        }
        if arg.starts_with("--date=")
            || arg.starts_with("--reference=")
            || arg.starts_with("--time=")
        {
            i += 1;
            continue;
        }
        if (arg.starts_with("-d") || arg.starts_with("-r") || arg.starts_with("-t"))
            && arg.len() > 2
        {
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            if matches!(
                arg.as_str(),
                "--no-create" | "--no-dereference" | "--access" | "--modification"
            ) {
                i += 1;
                continue;
            }
            unsupported_flags.push(arg.clone());
            i += 1;
            continue;
        }
        if is_short_flag_cluster(arg, &['a', 'c', 'h', 'm']) {
            i += 1;
            continue;
        }
        unsupported_flags.push(arg.clone());
        i += 1;
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported touch flags",
            unsupported_flags,
        ));
    }
    if targets.is_empty() {
        return Some(unknown_outcome("touch missing file operand"));
    }
    Some(known_fs_outcome(targets, Action::Write))
}

fn summarize_chmod(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "chmod") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        inner,
        &['R', 'v', 'c', 'f'],
        &[
            "--recursive",
            "--verbose",
            "--changes",
            "--silent",
            "--quiet",
        ],
        &[],
    );
    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported chmod flags",
            unsupported_flags,
        ));
    }
    if positionals.len() < 2 {
        return Some(unknown_outcome("chmod missing operand"));
    }
    Some(known_fs_outcome(positionals[1..].to_vec(), Action::Write))
}

fn summarize_chown(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "chown") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        inner,
        &['R', 'h', 'v', 'f', 'c', 'H', 'L', 'P'],
        &[
            "--recursive",
            "--no-dereference",
            "--verbose",
            "--silent",
            "--quiet",
            "--changes",
        ],
        &[],
    );
    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported chown flags",
            unsupported_flags,
        ));
    }
    if positionals.len() < 2 {
        return Some(unknown_outcome("chown missing operand"));
    }
    Some(known_fs_outcome(positionals[1..].to_vec(), Action::Write))
}

fn summarize_tee(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "tee") {
        return None;
    }

    let mut append = false;
    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;

    for arg in inner.iter().skip(1) {
        if saw_end_of_flags {
            targets.push(arg.clone());
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            targets.push(arg.clone());
            continue;
        }

        if arg.starts_with("--") {
            match arg.as_str() {
                "--append" => append = true,
                "--ignore-interrupts" => {}
                _ => unsupported_flags.push(arg.clone()),
            }
            continue;
        }

        if is_short_flag_cluster(arg, &['a', 'i']) {
            if arg[1..].contains('a') {
                append = true;
            }
            continue;
        }

        unsupported_flags.push(arg.clone());
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported tee flags",
            unsupported_flags,
        ));
    }
    if targets.is_empty() {
        return Some(known_outcome(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        }));
    }

    let action = if append {
        Action::Append
    } else {
        Action::Write
    };
    Some(known_fs_outcome(targets, action))
}

fn summarize_curl(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_curl_command(argv) {
        return None;
    }

    #[derive(Debug, Clone)]
    struct PayloadArg {
        flag: String,
        value_index: usize,
    }

    let mut method: Option<String> = None;
    let mut url_raw: Option<String> = None;
    let mut payloads: Vec<PayloadArg> = Vec::new();
    let mut unsupported_flags: Vec<String> = Vec::new();
    let mut i = 1usize;
    let mut saw_end_of_flags = false;

    while i < argv.len() {
        let arg = &argv[i];
        if !saw_end_of_flags && arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }

        if !saw_end_of_flags && arg.starts_with('-') {
            if arg == "-X" || arg == "--request" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl method flag missing value"));
                }
                method = Some(argv[i + 1].to_ascii_uppercase());
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--request=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl method flag missing value"));
                }
                method = Some(raw.to_ascii_uppercase());
                i += 1;
                continue;
            }

            if arg == "--url" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl url flag missing value"));
                }
                url_raw = Some(argv[i + 1].clone());
                i += 2;
                continue;
            }

            if let Some(raw) = arg.strip_prefix("--url=") {
                if raw.is_empty() {
                    return Some(unknown_outcome("curl url flag missing value"));
                }
                url_raw = Some(raw.to_string());
                i += 1;
                continue;
            }

            if arg == "-d"
                || arg == "--data"
                || arg == "--data-raw"
                || arg == "--data-binary"
                || arg == "--data-ascii"
                || arg == "--data-urlencode"
                || arg == "--json"
            {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl payload flag missing value"));
                }
                payloads.push(PayloadArg {
                    flag: arg.clone(),
                    value_index: i + 1,
                });
                i += 2;
                continue;
            }

            if arg.starts_with("-d") && arg.len() > 2 {
                payloads.push(PayloadArg {
                    flag: "-d".to_string(),
                    value_index: i,
                });
                i += 1;
                continue;
            }

            if let Some((flag, _value)) = split_flag_value(arg) {
                if matches!(
                    flag,
                    "--data"
                        | "--data-raw"
                        | "--data-binary"
                        | "--data-ascii"
                        | "--data-urlencode"
                        | "--json"
                ) {
                    payloads.push(PayloadArg {
                        flag: flag.to_string(),
                        value_index: i,
                    });
                    i += 1;
                    continue;
                }
            }

            if matches!(
                arg.as_str(),
                "-s" | "-S"
                    | "-L"
                    | "-k"
                    | "-f"
                    | "--silent"
                    | "--show-error"
                    | "--location"
                    | "--insecure"
                    | "--fail"
                    | "--fail-with-body"
            ) {
                i += 1;
                continue;
            }

            if is_short_flag_cluster(arg, &['s', 'S', 'L', 'k', 'f']) {
                i += 1;
                continue;
            }

            if arg == "-H" || arg == "--header" {
                if i + 1 >= argv.len() {
                    return Some(unknown_outcome("curl header flag missing value"));
                }
                i += 2;
                continue;
            }

            if arg.starts_with("--header=") {
                i += 1;
                continue;
            }

            unsupported_flags.push(arg.clone());
            i += 1;
            continue;
        }

        if url_raw.is_none() {
            url_raw = Some(arg.clone());
        }
        i += 1;
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported curl flags",
            unsupported_flags,
        ));
    }

    let method = method.unwrap_or_else(|| {
        if payloads.is_empty() {
            "GET".to_string()
        } else {
            "POST".to_string()
        }
    });
    if matches!(method.as_str(), "GET" | "HEAD") {
        let Some(url) = url_raw else {
            return Some(unknown_outcome("curl request missing URL"));
        };
        let Some(sink) = canonicalize_url_connect_scope(&url) else {
            return Some(unknown_outcome("curl request has invalid URL sink"));
        };
        return Some(known_outcome(CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: sink.0.clone(),
            }],
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        }));
    }
    if !matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return None;
    }

    let Some(url) = url_raw else {
        return Some(unknown_outcome("curl mutating request missing URL"));
    };
    let Some(sink) = canonicalize_url_sink(&url) else {
        return Some(unknown_outcome(
            "curl mutating request has invalid URL sink",
        ));
    };

    let sink_checks = payloads
        .into_iter()
        .map(|payload| SinkCheck {
            argument_name: payload.flag,
            sink: sink.clone(),
            value_refs: vec![ValueRef(format!("argv:{}", payload.value_index))],
        })
        .collect();

    Some(known_outcome(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: sink.0.clone(),
        }],
        sink_checks,
        unsupported_flags: Vec::new(),
    }))
}

fn split_flag_value(flag: &str) -> Option<(&str, &str)> {
    let eq = flag.find('=')?;
    Some((&flag[..eq], &flag[eq + 1..]))
}

fn collect_positionals_with_no_value_flags(
    argv: &[String],
    allowed_short_flags: &[char],
    allowed_long_flags: &[&str],
    allowed_long_prefixes: &[&str],
) -> (Vec<String>, Vec<String>) {
    let mut positionals = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;

    for arg in argv.iter().skip(1) {
        if saw_end_of_flags {
            positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            positionals.push(arg.clone());
            continue;
        }

        if arg.starts_with("--") {
            if allowed_long_flags.contains(&arg.as_str())
                || allowed_long_prefixes
                    .iter()
                    .any(|prefix| arg.starts_with(prefix))
            {
                continue;
            }
            unsupported_flags.push(arg.clone());
            continue;
        }

        if is_short_flag_cluster(arg, allowed_short_flags) {
            continue;
        }
        unsupported_flags.push(arg.clone());
    }

    (positionals, unsupported_flags)
}

fn known_fs_outcome(scopes: Vec<String>, action: Action) -> SummaryOutcome {
    known_outcome(CommandSummary {
        required_capabilities: scopes
            .into_iter()
            .map(|scope| Capability {
                resource: Resource::Fs,
                action,
                scope,
            })
            .collect(),
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn is_short_flag_cluster(arg: &str, allowed_flags: &[char]) -> bool {
    arg.len() > 1
        && arg.starts_with('-')
        && !arg.starts_with("--")
        && arg[1..].chars().all(|ch| allowed_flags.contains(&ch))
}

fn is_rm_command(argv: &[String]) -> bool {
    is_named_command(argv, "rm")
}

fn is_curl_command(argv: &[String]) -> bool {
    is_named_command(argv, "curl")
}

fn is_named_command(argv: &[String], command: &str) -> bool {
    basename(argv.first()).is_some_and(|cmd| cmd == command)
}

fn strip_sudo(argv: &[String]) -> &[String] {
    if basename(argv.first()).is_some_and(|cmd| cmd == "sudo") && argv.len() > 1 {
        &argv[1..]
    } else {
        argv
    }
}

fn basename(s: Option<&String>) -> Option<&str> {
    let s = s?;
    std::path::Path::new(s)
        .file_name()
        .and_then(|part| part.to_str())
}

pub(crate) fn canonicalize_url_connect_scope(raw: &str) -> Option<SinkKey> {
    let url = Url::parse(raw).ok()?;
    let scheme = url.scheme().to_ascii_lowercase();
    let host = match url.host()? {
        Host::Domain(domain) => domain.to_ascii_lowercase(),
        Host::Ipv4(addr) => addr.to_string(),
        Host::Ipv6(addr) => format!("[{addr}]"),
    };
    let port = url
        .port()
        .filter(|p| Some(*p) != default_port_for_scheme(&scheme));
    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push('/');
    Some(SinkKey(out))
}

fn canonicalize_url_sink(raw: &str) -> Option<SinkKey> {
    let url = Url::parse(raw).ok()?;
    let scheme = url.scheme().to_ascii_lowercase();
    let host = match url.host()? {
        Host::Domain(domain) => domain.to_ascii_lowercase(),
        Host::Ipv4(addr) => addr.to_string(),
        Host::Ipv6(addr) => format!("[{addr}]"),
    };
    let port = url
        .port()
        .filter(|p| Some(*p) != default_port_for_scheme(&scheme));
    let path = normalize_path(url.path());

    let mut out = format!("{scheme}://{host}");
    if let Some(port) = port {
        out.push(':');
        out.push_str(&port.to_string());
    }
    out.push_str(&path);
    Some(SinkKey(out))
}

fn default_port_for_scheme(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn normalize_path(path: &str) -> String {
    let has_trailing_slash = path.ends_with('/') && path != "/";
    let mut stack: Vec<String> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            _ => stack.push(normalize_percent_encoding(segment)),
        }
    }

    if stack.is_empty() {
        return "/".to_string();
    }

    let mut out = format!("/{}", stack.join("/"));
    if has_trailing_slash {
        out.push('/');
    }
    out
}

fn normalize_percent_encoding(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(decoded) = decode_hex_pair(bytes[i + 1], bytes[i + 2]) {
                if is_unreserved(decoded) {
                    out.push(decoded as char);
                } else {
                    out.push('%');
                    out.push(to_upper_hex(decoded >> 4));
                    out.push(to_upper_hex(decoded & 0x0f));
                }
                i += 3;
                continue;
            }
        }

        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn decode_hex_pair(high: u8, low: u8) -> Option<u8> {
    Some((from_hex(high)? << 4) | from_hex(low)?)
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn to_upper_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => unreachable!("nibble out of range"),
    }
}

fn is_unreserved(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
    )
}

fn known_outcome(summary: CommandSummary) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Known,
        summary: Some(summary),
        reason: None,
    }
}

fn unknown_outcome(reason: &str) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Unknown,
        summary: None,
        reason: Some(reason.to_string()),
    }
}

fn unknown_with_flags(reason: &str, unsupported_flags: Vec<String>) -> SummaryOutcome {
    SummaryOutcome {
        knowledge: CommandKnowledge::Unknown,
        summary: Some(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags,
        }),
        reason: Some(reason.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn rm_rf_maps_to_fs_write_capability() {
        let out = summarize_argv(&argv(&["rm", "-rf", "/tmp/demo"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.required_capabilities.len(), 1);
        assert_eq!(
            summary.required_capabilities[0],
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/demo".to_string()
            }
        );
        assert!(summary.sink_checks.is_empty());
    }

    #[test]
    fn rm_unknown_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&["rm", "-rfv", "/tmp/demo"]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out
            .summary
            .expect("expected summary with unsupported flags");
        assert_eq!(summary.unsupported_flags, vec!["-rfv".to_string()]);
    }

    #[test]
    fn cp_maps_destination_to_fs_write_capability() {
        let out = summarize_argv(&argv(&["cp", "a.txt", "b.txt"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "b.txt".to_string(),
            }]
        );
    }

    #[test]
    fn mv_maps_source_and_destination_to_fs_write_capability() {
        let out = summarize_argv(&argv(&["mv", "a.txt", "b.txt"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "a.txt".to_string(),
                },
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "b.txt".to_string(),
                }
            ]
        );
    }

    #[test]
    fn mkdir_mode_and_parents_flags_are_supported() {
        let out = summarize_argv(&argv(&["mkdir", "-p", "-m", "755", "tmp/work"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "tmp/work".to_string(),
            }]
        );
    }

    #[test]
    fn touch_with_time_flag_maps_to_fs_write() {
        let out = summarize_argv(&argv(&["touch", "-d", "2026-01-01", "file.txt"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "file.txt".to_string(),
            }]
        );
    }

    #[test]
    fn chmod_maps_targets_to_fs_write() {
        let out = summarize_argv(&argv(&["chmod", "-R", "755", "bin", "out"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "bin".to_string(),
                },
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "out".to_string(),
                }
            ]
        );
    }

    #[test]
    fn chown_unsupported_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&[
            "chown",
            "--from=user:group",
            "root:root",
            "file.txt",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.unsupported_flags,
            vec!["--from=user:group".to_string()]
        );
    }

    #[test]
    fn tee_append_maps_to_fs_append_capability() {
        let out = summarize_argv(&argv(&["tee", "-a", "audit.log"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Fs,
                action: Action::Append,
                scope: "audit.log".to_string(),
            }]
        );
    }

    #[test]
    fn curl_post_url_requires_net_write_no_payload_sink_checks() {
        let out = summarize_argv(&argv(&["curl", "-X", "POST", "https://api.example.com/v1"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1".to_string()
            }]
        );
        assert!(summary.sink_checks.is_empty());
    }

    #[test]
    fn curl_post_with_payload_extracts_sink_check() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://api.example.com/v1/upload",
            "-d",
            "{\"k\":\"v\"}",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.sink_checks.len(), 1);
        assert_eq!(summary.sink_checks[0].argument_name, "-d");
        assert_eq!(
            summary.sink_checks[0].sink,
            SinkKey("https://api.example.com/v1/upload".to_string())
        );
        assert_eq!(
            summary.sink_checks[0].value_refs,
            vec![ValueRef("argv:5".to_string())]
        );
    }

    #[test]
    fn curl_payload_without_explicit_method_defaults_to_post() {
        let out = summarize_argv(&argv(&[
            "curl",
            "https://api.example.com/v1/upload",
            "--data",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.required_capabilities.len(), 1);
        assert_eq!(
            summary.required_capabilities[0].scope,
            "https://api.example.com/v1/upload".to_string()
        );
        assert_eq!(summary.sink_checks[0].argument_name, "--data");
    }

    #[test]
    fn curl_put_with_payload_extracts_sink_check() {
        let out = summarize_argv(&argv(&[
            "curl",
            "--request",
            "put",
            "--url",
            "https://api.example.com/v1/upload",
            "--data-binary",
            "blob",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.sink_checks.len(), 1);
        assert_eq!(summary.sink_checks[0].argument_name, "--data-binary");
        assert_eq!(
            summary.sink_checks[0].sink,
            SinkKey("https://api.example.com/v1/upload".to_string())
        );
    }

    #[test]
    fn curl_header_flag_missing_value_routes_to_unknown() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://api.example.com/v1/upload",
            "-H",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(
            out.reason.as_deref(),
            Some("curl header flag missing value")
        );
    }

    #[test]
    fn safe_read_command_is_known_with_empty_summary() {
        let out = summarize_argv(&argv(&["ls", "-la"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert!(summary.required_capabilities.is_empty());
        assert!(summary.sink_checks.is_empty());
        assert!(summary.unsupported_flags.is_empty());
    }

    #[test]
    fn curl_unknown_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "--upload-file",
            "payload.bin",
            "https://api.example.com/v1/upload",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.unsupported_flags, vec!["--upload-file".to_string()]);
    }

    #[test]
    fn curl_short_upload_file_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "PUT",
            "-T",
            "payload.bin",
            "https://api.example.com/v1/upload",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.unsupported_flags, vec!["-T".to_string()]);
    }

    #[test]
    fn curl_multipart_form_flag_routes_to_unknown() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "-F",
            "file=@payload.bin",
            "https://api.example.com/v1/upload",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.unsupported_flags, vec!["-F".to_string()]);
    }

    #[test]
    fn curl_post_url_sink_is_canonicalized() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "HTTPS://API.Example.COM:443/a/../b/./c%7e?x=1#frag",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        let expected = "https://api.example.com/b/c~".to_string();
        assert_eq!(summary.required_capabilities[0].scope, expected);
        assert_eq!(summary.sink_checks[0].sink, SinkKey(expected));
    }

    #[test]
    fn curl_post_non_default_port_is_preserved() {
        let out = summarize_argv(&argv(&[
            "curl",
            "--request=POST",
            "https://api.example.com:8443/v1/upload",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities[0].scope,
            "https://api.example.com:8443/v1/upload".to_string()
        );
    }

    #[test]
    fn curl_post_ipv6_sink_keeps_brackets() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://[2001:DB8::1]:443/a/./b/../c",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        let expected = "https://[2001:db8::1]/a/c".to_string();
        assert_eq!(summary.required_capabilities[0].scope, expected);
        assert_eq!(summary.sink_checks[0].sink, SinkKey(expected));
    }

    #[test]
    fn curl_post_idn_host_is_normalized_to_ascii() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://BÜCHER.example/%C3%BCber",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        let expected = "https://xn--bcher-kva.example/%C3%BCber".to_string();
        assert_eq!(summary.required_capabilities[0].scope, expected);
    }

    #[test]
    fn curl_get_url_requires_net_connect_capability() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-sS",
            "https://api.open-meteo.com/v1/forecast?latitude=1&longitude=2",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.sink_checks, Vec::<SinkCheck>::new());
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://api.open-meteo.com/".to_string(),
            }]
        );
    }

    #[test]
    fn curl_get_connect_scope_keeps_non_default_port() {
        let out = summarize_argv(&argv(&["curl", "https://example.com:8443/path?q=1"]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities,
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "https://example.com:8443/".to_string(),
            }]
        );
    }

    #[test]
    fn curl_get_missing_url_routes_to_unknown() {
        let out = summarize_argv(&argv(&["curl", "-sS"]));

        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(out.reason.as_deref(), Some("curl request missing URL"));
    }

    #[test]
    fn curl_post_encoded_slash_stays_encoded() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://api.example.com/a%2fb?debug=1",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        let expected = "https://api.example.com/a%2Fb".to_string();
        assert_eq!(summary.required_capabilities[0].scope, expected);
    }

    #[test]
    fn curl_post_host_without_path_normalizes_to_root_path() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://api.example.com",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities[0].scope,
            "https://api.example.com/".to_string()
        );
    }

    #[test]
    fn curl_post_dot_segment_with_trailing_slash_is_preserved() {
        let out = summarize_argv(&argv(&[
            "curl",
            "-X",
            "POST",
            "https://api.example.com/a/b/../",
            "-d",
            "body",
        ]));

        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(
            summary.required_capabilities[0].scope,
            "https://api.example.com/a/".to_string()
        );
    }

    #[test]
    fn codex_safe_bash_lc_class_is_known() {
        let out = summarize_argv(&argv(&["bash", "-lc", "ls && cat Cargo.toml"]));
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert!(summary.required_capabilities.is_empty());
    }

    #[test]
    fn codex_dangerous_bash_lc_class_routes_to_unknown() {
        let out = summarize_argv(&argv(&["bash", "-lc", "rm -rf /tmp/demo"]));
        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(
            out.reason.as_deref(),
            Some("dangerous command class lacks explicit summary")
        );
    }

    #[test]
    fn rm_f_routes_to_dangerous_unknown() {
        let out = summarize_argv(&argv(&["rm", "-f", "/tmp/demo"]));
        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(
            out.reason.as_deref(),
            Some("dangerous command class lacks explicit summary")
        );
    }

    #[test]
    fn planner_command_catalog_includes_bravesearch_entry() {
        assert!(planner_command_catalog().iter().any(|entry| {
            entry.command == "bravesearch" && entry.description.contains("Search Brave index")
        }));
    }

    #[test]
    fn planner_command_catalog_bravesearch_mentions_discovery_followup() {
        let entry = planner_command_catalog()
            .iter()
            .find(|entry| entry.command == "bravesearch")
            .expect("bravesearch catalog entry");
        assert!(entry.description.contains("After discovery"));
        assert!(entry.description.contains("curl"));
    }

    #[test]
    fn planner_command_catalog_curl_mentions_markdown_new() {
        let entry = planner_command_catalog()
            .iter()
            .find(|entry| entry.command == "curl")
            .expect("curl catalog entry");
        assert!(entry.description.contains("markdown.new"));
    }

    #[test]
    fn planner_command_catalog_includes_codex_exec_entry() {
        let entry = planner_command_catalog()
            .iter()
            .find(|entry| entry.command == "codex")
            .expect("codex catalog entry");
        assert!(entry.description.contains("codex exec"));
    }

    #[test]
    fn planner_command_catalog_codex_mentions_read_only_and_workspace_write() {
        let entry = planner_command_catalog()
            .iter()
            .find(|entry| entry.command == "codex")
            .expect("codex catalog entry");
        assert!(entry.description.contains("--sandbox read-only"));
        assert!(entry.description.contains("--sandbox workspace-write"));
        assert!(entry.description.contains("--ephemeral"));
    }

    #[test]
    fn planner_command_catalog_includes_sieve_lcm_cli_entry() {
        let entry = planner_command_catalog()
            .iter()
            .find(|entry| entry.command == "sieve-lcm-cli")
            .expect("sieve-lcm-cli catalog entry");
        assert!(entry.description.contains("query --lane both"));
        assert!(entry.description.contains("expand --ref"));
    }

    #[test]
    fn sieve_lcm_cli_query_is_known_no_capabilities() {
        let out = summarize_argv(&argv(&[
            "sieve-lcm-cli",
            "query",
            "--lane",
            "both",
            "--query",
            "where do i live",
            "--json",
        ]));
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert!(summary.required_capabilities.is_empty());
    }

    #[test]
    fn sieve_lcm_cli_ingest_requires_fs_write_capability() {
        let out = summarize_argv(&argv(&[
            "sieve-lcm-cli",
            "ingest",
            "--db",
            "/tmp/memory.db",
            "--conversation",
            "global",
            "--role",
            "user",
            "--content",
            "hello",
        ]));
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("expected summary");
        assert_eq!(summary.required_capabilities.len(), 1);
        assert_eq!(summary.required_capabilities[0].resource, Resource::Fs);
        assert_eq!(summary.required_capabilities[0].action, Action::Write);
        assert_eq!(
            summary.required_capabilities[0].scope,
            "/tmp/memory.db".to_string()
        );
    }
}
