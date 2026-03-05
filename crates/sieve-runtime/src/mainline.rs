use super::{RuntimeDisposition, RuntimeError, RuntimeOrchestrator};
use async_trait::async_trait;
use sieve_types::{CommandSegment, RunId};
use thiserror::Error;
use tokio::process::Command as TokioCommand;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub script: String,
    pub command_segments: Vec<CommandSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainlineArtifactKind {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineArtifact {
    pub ref_id: String,
    pub kind: MainlineArtifactKind,
    pub path: String,
    pub byte_count: u64,
    pub line_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineRunReport {
    pub run_id: RunId,
    pub exit_code: Option<i32>,
    pub artifacts: Vec<MainlineArtifact>,
}

#[derive(Debug, Error)]
pub enum MainlineRunError {
    #[error("mainline command execution failed: {0}")]
    Exec(String),
}

#[async_trait]
pub trait MainlineRunner: Send + Sync {
    async fn run(&self, request: MainlineRunRequest)
        -> Result<MainlineRunReport, MainlineRunError>;
}

pub struct BashMainlineRunner;

#[async_trait]
impl MainlineRunner for BashMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        let status = TokioCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .status()
            .await
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;
        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: status.code(),
            artifacts: Vec::new(),
        })
    }
}

impl RuntimeOrchestrator {
    pub(super) async fn execute_mainline(
        &self,
        run_id: RunId,
        cwd: String,
        script: String,
        command_segments: Vec<CommandSegment>,
    ) -> Result<RuntimeDisposition, RuntimeError> {
        let report = self
            .mainline
            .run(MainlineRunRequest {
                run_id,
                cwd,
                script,
                command_segments,
            })
            .await?;
        Ok(RuntimeDisposition::ExecuteMainline(report))
    }
}
