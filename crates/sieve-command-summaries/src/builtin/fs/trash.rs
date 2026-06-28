use sieve_types::Action;

use crate::{
    is_named_command, is_short_flag_cluster, known_empty_outcome, known_fs_outcome,
    split_flag_value, unknown_outcome, unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_trash(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_named_command(argv, "trash") {
        return None;
    }

    let mut saw_meta_only_flag = false;
    let mut saw_end_of_flags = false;
    let mut trash_dir = None;
    let mut targets = Vec::new();
    let mut unsupported_flags = Vec::new();
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
                let Some(value) = argv.get(i + 1) else {
                    return Some(unknown_outcome("trash --trash-dir missing value"));
                };
                trash_dir = Some(value.clone());
                i += 2;
            }
            "--print-completion" => {
                let Some(value) = argv.get(i + 1) else {
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
        return Some(known_empty_outcome());
    }

    if targets.is_empty() {
        return Some(unknown_outcome("trash missing target"));
    }

    if let Some(trash_dir) = trash_dir {
        targets.push(trash_dir);
    }

    Some(known_fs_outcome(targets, Action::Write))
}
