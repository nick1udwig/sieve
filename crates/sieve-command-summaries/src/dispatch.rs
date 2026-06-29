use codex_shell_command::command_safety::is_dangerous_command::command_might_be_dangerous;
use codex_shell_command::command_safety::is_safe_command::is_known_safe_command;

use crate::{
    agent_browser, brave_search, builtin, curl, gws, known_empty_outcome, rebase_argv_value_refs,
    st, strip_env_wrapper_with_offset, strip_sudo_with_offset, unknown_outcome, SummaryOutcome,
};

pub(crate) fn summarize_argv(argv: &[String]) -> SummaryOutcome {
    if argv.is_empty() {
        return unknown_outcome("empty argv");
    }

    let sudo_stripped = strip_sudo_with_offset(argv);
    if sudo_stripped.offset > 0 {
        return rebase_argv_value_refs(summarize_argv(sudo_stripped.argv), sudo_stripped.offset);
    }

    let env_stripped = strip_env_wrapper_with_offset(argv);
    if env_stripped.offset > 0 {
        return rebase_argv_value_refs(summarize_argv(env_stripped.argv), env_stripped.offset);
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
