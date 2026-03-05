use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::{
    collect_positionals_with_no_value_flags, flag_value, is_named_command, is_short_flag_cluster,
    known_fs_outcome, known_outcome, strip_sudo, unknown_outcome, unknown_with_flags,
    SummaryOutcome,
};

pub(crate) fn summarize_builtin(argv: &[String]) -> Option<SummaryOutcome> {
    summarize_rm(argv)
        .or_else(|| summarize_cp(argv))
        .or_else(|| summarize_mv(argv))
        .or_else(|| summarize_mkdir(argv))
        .or_else(|| summarize_touch(argv))
        .or_else(|| summarize_chmod(argv))
        .or_else(|| summarize_chown(argv))
        .or_else(|| summarize_tee(argv))
        .or_else(|| summarize_sieve_lcm_cli(argv))
}

fn summarize_rm(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "rm") {
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
