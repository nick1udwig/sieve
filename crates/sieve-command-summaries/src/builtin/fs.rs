use sieve_types::{Action, CommandSummary};

use crate::{
    collect_positionals_with_no_value_flags, is_named_command, is_short_flag_cluster,
    known_fs_outcome, known_outcome, split_flag_value, strip_sudo, unknown_outcome,
    unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_fs_builtin(argv: &[String]) -> Option<SummaryOutcome> {
    summarize_trash(argv)
        .or_else(|| summarize_cp(argv))
        .or_else(|| summarize_mv(argv))
        .or_else(|| summarize_mkdir(argv))
        .or_else(|| summarize_touch(argv))
        .or_else(|| summarize_chmod(argv))
        .or_else(|| summarize_chown(argv))
        .or_else(|| summarize_tee(argv))
}

fn summarize_trash(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = strip_sudo(argv);
    if !is_named_command(inner, "trash") {
        return None;
    }

    let mut saw_meta_only_flag = false;
    let mut saw_end_of_flags = false;
    let mut trash_dir = None;
    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
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

        match arg.as_str() {
            "-h" | "--help" | "--version" => {
                saw_meta_only_flag = true;
                i += 1;
            }
            "-d" | "--directory" | "-f" | "--force" | "-i" | "--interactive" | "-r" | "-R"
            | "--recursive" | "-v" | "--verbose" => {
                i += 1;
            }
            "--trash-dir" => {
                let Some(value) = inner.get(i + 1) else {
                    return Some(unknown_outcome("trash --trash-dir missing value"));
                };
                trash_dir = Some(value.clone());
                i += 2;
            }
            "--print-completion" => {
                let Some(value) = inner.get(i + 1) else {
                    return Some(unknown_outcome("trash --print-completion missing value"));
                };
                if !matches!(value.as_str(), "bash" | "zsh" | "tcsh") {
                    return Some(unknown_outcome("trash --print-completion invalid shell"));
                }
                saw_meta_only_flag = true;
                i += 2;
            }
            _ if is_short_flag_cluster(arg, &['d', 'f', 'h', 'i', 'r', 'R', 'v']) => {
                if arg.contains('h') {
                    saw_meta_only_flag = true;
                }
                i += 1;
            }
            _ => {
                if let Some((flag, value)) = split_flag_value(arg) {
                    match flag {
                        "--trash-dir" if !value.is_empty() => {
                            trash_dir = Some(value.to_string());
                            i += 1;
                            continue;
                        }
                        "--print-completion" if !value.is_empty() => {
                            if !matches!(value, "bash" | "zsh" | "tcsh") {
                                return Some(unknown_outcome(
                                    "trash --print-completion invalid shell",
                                ));
                            }
                            saw_meta_only_flag = true;
                            i += 1;
                            continue;
                        }
                        _ => {}
                    }
                }
                unsupported_flags.push(arg.clone());
                i += 1;
            }
        }
    }

    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(
            "unsupported trash flags",
            unsupported_flags,
        ));
    }

    if saw_meta_only_flag {
        if trash_dir.is_some() || !targets.is_empty() {
            return Some(unknown_outcome(
                "trash metadata flags cannot be combined with file targets",
            ));
        }
        return Some(known_outcome(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        }));
    }

    if targets.is_empty() {
        return Some(unknown_outcome("trash missing target"));
    }

    if let Some(trash_dir) = trash_dir {
        targets.push(trash_dir);
    }

    Some(known_fs_outcome(targets, Action::Write))
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
