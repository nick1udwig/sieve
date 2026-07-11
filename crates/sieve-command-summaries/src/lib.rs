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
pub use catalog::{
    command_available, planner_available_command_catalog, planner_command_catalog, resolve_command,
    PlannerCommandDescriptor,
};

pub(crate) use common::{
    basename, collect_positionals_with_no_value_flags, flag_value, is_curl_command,
    is_named_command, is_short_flag_cluster, known_capabilities_outcome, known_empty_outcome,
    known_fs_outcome, known_summary_outcome, rebase_argv_value_refs, split_flag_value,
    unknown_outcome, unknown_with_flags,
};
pub(crate) use curl::canonicalize_url_connect_scope;
pub(crate) use dispatch::summarize_argv;

#[cfg(test)]
mod tests;
