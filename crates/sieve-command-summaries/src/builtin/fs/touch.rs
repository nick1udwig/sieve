use sieve_types::{Action, Capability, Resource};

use crate::{
    is_named_command, is_short_flag_cluster, known_capabilities_outcome, known_fs_outcome,
    unknown_outcome, unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_touch(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_named_command(argv, "touch") {
        return None;
    }

    let mut targets = Vec::new();
    let mut reference_scopes = Vec::new();
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

        if matches!(arg.as_str(), "-r" | "--reference") {
            if i + 1 >= argv.len() {
                return Some(unknown_outcome("touch time/reference flag missing value"));
            }
            reference_scopes.push(argv[i + 1].clone());
            i += 2;
            continue;
        }
        if matches!(arg.as_str(), "-d" | "-t" | "--date" | "--time") {
            if i + 1 >= argv.len() {
                return Some(unknown_outcome("touch time/reference flag missing value"));
            }
            i += 2;
            continue;
        }
        if matches!(arg.as_str(), "--date=" | "--reference=" | "--time=") {
            return Some(unknown_outcome("touch time/reference flag missing value"));
        }
        if let Some(reference_scope) = arg.strip_prefix("--reference=") {
            reference_scopes.push(reference_scope.to_string());
            i += 1;
            continue;
        }
        if arg.starts_with("--date=") || arg.starts_with("--time=") {
            i += 1;
            continue;
        }
        if let Some(reference_scope) = arg.strip_prefix("-r").filter(|value| !value.is_empty()) {
            reference_scopes.push(reference_scope.to_string());
            i += 1;
            continue;
        }
        if (arg.starts_with("-d") || arg.starts_with("-t")) && arg.len() > 2 {
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

    if reference_scopes.is_empty() {
        return Some(known_fs_outcome(targets, Action::Write));
    }

    let mut required_capabilities = Vec::with_capacity(reference_scopes.len() + targets.len());
    required_capabilities.extend(reference_scopes.into_iter().map(|scope| Capability {
        resource: Resource::Fs,
        action: Action::Read,
        scope,
    }));
    required_capabilities.extend(targets.into_iter().map(|scope| Capability {
        resource: Resource::Fs,
        action: Action::Write,
        scope,
    }));

    Some(known_capabilities_outcome(required_capabilities))
}
