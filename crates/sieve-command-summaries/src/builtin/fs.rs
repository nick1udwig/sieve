use sieve_types::{Action, CommandSummary};

use crate::{
    collect_positionals_with_no_value_flags, is_named_command, is_short_flag_cluster,
    known_fs_outcome, known_outcome, unknown_outcome, unknown_with_flags, SummaryOutcome,
};

mod cp;
mod mkdir;
mod touch;
mod trash;

use cp::summarize_cp;
use mkdir::summarize_mkdir;
use touch::summarize_touch;
use trash::summarize_trash;

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

fn summarize_mv(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_named_command(argv, "mv") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        argv,
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

fn summarize_chmod(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_named_command(argv, "chmod") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        argv,
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
    if !is_named_command(argv, "chown") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        argv,
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
    if !is_named_command(argv, "tee") {
        return None;
    }

    let mut append = false;
    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;

    for arg in argv.iter().skip(1) {
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
