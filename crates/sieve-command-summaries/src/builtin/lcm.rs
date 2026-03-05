use sieve_types::{Action, CommandSummary};

use crate::{
    flag_value, is_named_command, known_fs_outcome, known_outcome, strip_sudo, unknown_outcome,
    SummaryOutcome,
};

pub(super) fn summarize_sieve_lcm_cli(argv: &[String]) -> Option<SummaryOutcome> {
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
