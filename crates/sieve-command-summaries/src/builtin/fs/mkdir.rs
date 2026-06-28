use sieve_types::Action;

use crate::{
    is_named_command, is_short_flag_cluster, known_fs_outcome, unknown_outcome, unknown_with_flags,
    SummaryOutcome,
};

pub(super) fn summarize_mkdir(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_named_command(argv, "mkdir") {
        return None;
    }

    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
    let mut saw_end_of_flags = false;
    let mut i = 1usize;

    while i < argv.len() {
        let arg = &argv[i];
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
            if i + 1 >= argv.len() {
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
