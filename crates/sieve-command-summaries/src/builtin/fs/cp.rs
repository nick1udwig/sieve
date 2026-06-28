use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::{
    collect_positionals_with_no_value_flags, is_named_command, known_outcome, unknown_outcome,
    unknown_with_flags, SummaryOutcome,
};

pub(super) fn summarize_cp(argv: &[String]) -> Option<SummaryOutcome> {
    if !is_named_command(argv, "cp") {
        return None;
    }

    let (positionals, unsupported_flags) = collect_positionals_with_no_value_flags(
        argv,
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
    let mut required_capabilities = Vec::with_capacity(positionals.len());
    required_capabilities.extend(positionals[..positionals.len() - 1].iter().map(|scope| {
        Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: scope.clone(),
        }
    }));
    required_capabilities.push(Capability {
        resource: Resource::Fs,
        action: Action::Write,
        scope: destination,
    });
    Some(known_outcome(CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    }))
}
