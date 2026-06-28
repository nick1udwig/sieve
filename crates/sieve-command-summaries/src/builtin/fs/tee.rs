use sieve_types::Action;

use crate::{
    is_named_command, is_short_flag_cluster, known_empty_outcome, known_fs_outcome,
    unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_tee(argv: &[String]) -> Option<SummaryOutcome> {
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
        return Some(known_empty_outcome());
    }

    let action = if append {
        Action::Append
    } else {
        Action::Write
    };
    Some(known_fs_outcome(targets, action))
}
