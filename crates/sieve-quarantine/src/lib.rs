#![forbid(unsafe_code)]

mod report;
mod runner;
mod trace;

#[cfg(test)]
mod tests;

use thiserror::Error;

#[cfg(test)]
pub(crate) use report::REPORT_FILE_NAME;
#[cfg(test)]
pub(crate) use runner::command_segments_to_script;
pub use runner::{BwrapQuarantineRunner, QuarantineNetworkMode, QuarantineRunner};
#[cfg(test)]
pub(crate) use trace::{collect_trace_files, parse_trace_capabilities, parse_trace_line};

#[derive(Debug, Error)]
pub enum QuarantineRunError {
    #[error("sandbox execution failed: {0}")]
    Exec(String),
}
