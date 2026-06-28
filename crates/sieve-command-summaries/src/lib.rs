#![forbid(unsafe_code)]

mod agent_browser;
mod api;
mod builtin;
mod catalog;
mod common;
mod dispatch;
mod gws;

#[path = "brave-search.rs"]
mod brave_search;
mod curl;
mod st;

pub use api::{CommandSummarizer, DefaultCommandSummarizer, SummaryOutcome};
pub use catalog::{planner_command_catalog, PlannerCommandDescriptor};

pub(crate) use common::{
    basename, collect_positionals_with_no_value_flags, flag_value, is_curl_command,
    is_named_command, is_short_flag_cluster, known_fs_outcome, known_outcome,
    rebase_argv_value_refs, split_flag_value, strip_env_assignments_with_offset,
    strip_sudo_with_offset, unknown_outcome, unknown_with_flags,
};
pub(crate) use curl::canonicalize_url_connect_scope;
pub(crate) use dispatch::summarize_argv;

#[cfg(test)]
mod tests;
