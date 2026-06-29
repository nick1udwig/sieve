use codex_shell_command::command_safety::is_dangerous_command::command_might_be_dangerous;
use codex_shell_command::command_safety::is_safe_command::is_known_safe_command;
use sieve_shell::strip_command_wrappers;

use crate::{
    agent_browser, brave_search, builtin, curl, gws, known_empty_outcome, rebase_argv_value_refs,
    st, unknown_outcome, SummaryOutcome,
};

pub(crate) fn summarize_argv(argv: &[String]) -> SummaryOutcome {
    if argv.is_empty() {
        return unknown_outcome("empty argv");
    }

    let stripped = strip_command_wrappers(argv);
    if stripped.offset > 0 {
        return rebase_argv_value_refs(summarize_argv(stripped.argv), stripped.offset);
    }

    if let Some(outcome) = builtin::summarize_builtin(argv) {
        return outcome;
    }

    if let Some(outcome) = curl::summarize_curl(argv) {
        return outcome;
    }

    if let Some(outcome) = agent_browser::summarize_agent_browser(argv) {
        return outcome;
    }

    if let Some(outcome) = gws::summarize_gws(argv) {
        return outcome;
    }

    if let Some(outcome) = st::summarize_st(argv) {
        return outcome;
    }

    if let Some(outcome) = brave_search::summarize_brave_search(argv) {
        return outcome;
    }

    if is_known_safe_command(argv) {
        return known_empty_outcome();
    }

    if command_might_be_dangerous(argv) {
        return unknown_outcome("dangerous command class lacks explicit summary");
    }

    unknown_outcome("unknown command")
}
