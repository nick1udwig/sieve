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
        browser_session_mutations: crate::browser_sessions::BrowserSessionMutations,
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
        if report.exit_code == Some(0) && !browser_session_mutations.is_empty() {
            let mut sessions = self
                .browser_sessions
                .lock()
                .map_err(|_| crate::ValueStateError::LockPoisoned)?;
            for (name, state) in browser_session_mutations {
                match state {
                    Some(state) => {
                        sessions.insert(name, state);
                    }
                    None => {
                        sessions.remove(&name);
                    }
                }
            }
        }
        Ok(RuntimeDisposition::ExecuteMainline(report))
    }
}
