#![forbid(unsafe_code)]

mod api;
mod builtin;
mod catalog;
mod common;
mod dispatch;

#[path = "brave-search.rs"]
mod brave_search;
mod codex;
mod curl;
mod st;

pub use api::{CommandSummarizer, DefaultCommandSummarizer, SummaryOutcome};
pub use catalog::{planner_command_catalog, PlannerCommandDescriptor};

pub(crate) use common::{
    basename, collect_positionals_with_no_value_flags, flag_value, is_curl_command,
    is_named_command, is_short_flag_cluster, known_fs_outcome, known_outcome, split_flag_value,
    strip_sudo, unknown_outcome, unknown_with_flags,
};
pub(crate) use curl::canonicalize_url_connect_scope;
pub(crate) use dispatch::summarize_argv;

#[cfg(test)]
mod tests;
