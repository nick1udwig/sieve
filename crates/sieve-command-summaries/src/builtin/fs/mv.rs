use sieve_types::Action;

use crate::{
    collect_positionals_with_no_value_flags, is_named_command, known_fs_outcome, unknown_outcome,
    unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_mv(argv: &[String]) -> Option<SummaryOutcome> {
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
