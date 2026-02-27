#![forbid(unsafe_code)]

use sieve_llm::LlmError;
use sieve_quarantine::QuarantineRunError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CapTraceError {
    #[error("invalid args: {0}")]
    Args(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("llm error: {0}")]
    Llm(String),
    #[error("shell parse error: {0}")]
    Shell(String),
    #[error("trace error: {0}")]
    Trace(String),
}

pub(crate) fn io_err(err: std::io::Error) -> CapTraceError {
    CapTraceError::Io(err.to_string())
}

pub(crate) fn llm_err(err: LlmError) -> CapTraceError {
    CapTraceError::Llm(err.to_string())
}

pub(crate) fn trace_err(err: QuarantineRunError) -> CapTraceError {
    CapTraceError::Trace(err.to_string())
}
