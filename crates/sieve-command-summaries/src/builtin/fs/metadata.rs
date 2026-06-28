use sieve_types::Action;

use crate::{
    collect_positionals_with_no_value_flags, is_named_command, known_fs_outcome, unknown_outcome,
    unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_chmod(argv: &[String]) -> Option<SummaryOutcome> {
    summarize_metadata_command(
        argv,
        "chmod",
        &['R', 'v', 'c', 'f'],
        &[
            "--recursive",
            "--verbose",
            "--changes",
            "--silent",
            "--quiet",
        ],
        "unsupported chmod flags",
        "chmod missing operand",
    )
}

pub(super) fn summarize_chown(argv: &[String]) -> Option<SummaryOutcome> {
    summarize_metadata_command(
        argv,
        "chown",
        &['R', 'h', 'v', 'f', 'c', 'H', 'L', 'P'],
        &[
            "--recursive",
            "--no-dereference",
            "--verbose",
            "--silent",
            "--quiet",
            "--changes",
        ],
        "unsupported chown flags",
        "chown missing operand",
    )
}

fn summarize_metadata_command(
    argv: &[String],
    command: &str,
    allowed_short_flags: &[char],
    allowed_long_flags: &[&str],
    unsupported_reason: &str,
    missing_operand_reason: &str,
) -> Option<SummaryOutcome> {
    if !is_named_command(argv, command) {
        return None;
    }

    let (positionals, unsupported_flags) =
        collect_positionals_with_no_value_flags(argv, allowed_short_flags, allowed_long_flags, &[]);
    if !unsupported_flags.is_empty() {
        return Some(unknown_with_flags(unsupported_reason, unsupported_flags));
    }
    if positionals.len() < 2 {
        return Some(unknown_outcome(missing_operand_reason));
    }
    Some(known_fs_outcome(positionals[1..].to_vec(), Action::Write))
}
