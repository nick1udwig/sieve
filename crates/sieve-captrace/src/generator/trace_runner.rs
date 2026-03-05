use crate::error::{trace_err, CapTraceError};
use async_trait::async_trait;
use sieve_quarantine::{BwrapQuarantineRunner, QuarantineNetworkMode, QuarantineRunner};
use sieve_types::{CommandSegment, QuarantineReport, QuarantineRunRequest, RunId};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct TraceRequest {
    pub run_id: String,
    pub cwd: String,
    pub argv: Vec<String>,
}

#[async_trait]
pub trait TraceRunner: Send + Sync {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError>;
}

#[derive(Clone)]
pub struct BwrapTraceRunner {
    inner: BwrapQuarantineRunner,
}

impl BwrapTraceRunner {
    pub fn new(logs_root: PathBuf) -> Self {
        Self {
            inner: BwrapQuarantineRunner::new(logs_root),
        }
    }

    pub fn with_sandbox(
        logs_root: PathBuf,
        network_mode: QuarantineNetworkMode,
        writable_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            inner: BwrapQuarantineRunner::with_sandbox(logs_root, network_mode, writable_paths),
        }
    }
}

#[async_trait]
impl TraceRunner for BwrapTraceRunner {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError> {
        let report = self
            .inner
            .run(QuarantineRunRequest {
                run_id: RunId(request.run_id),
                cwd: request.cwd,
                command_segments: vec![CommandSegment {
                    argv: request.argv,
                    operator_before: None,
                }],
            })
            .await
            .map_err(trace_err)?;
        Ok(report)
    }
}
