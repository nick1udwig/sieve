use crate::logging::now_ms;
use async_trait::async_trait;
use sieve_runtime::{
    MainlineArtifact, MainlineArtifactKind, MainlineRunError, MainlineRunReport,
    MainlineRunRequest, MainlineRunner,
};
use sieve_types::RunId;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::process::Command as TokioCommand;

pub(crate) struct AppMainlineRunner {
    artifact_root: PathBuf,
    next_artifact_id: AtomicU64,
}

impl AppMainlineRunner {
    pub(crate) fn new(artifact_root: PathBuf) -> Self {
        Self {
            artifact_root,
            next_artifact_id: AtomicU64::new(1),
        }
    }

    fn next_ref_id(&self) -> String {
        let next = self.next_artifact_id.fetch_add(1, Ordering::Relaxed);
        format!("artifact-{}-{next}", now_ms())
    }

    async fn persist_artifact(
        &self,
        run_id: &RunId,
        kind: MainlineArtifactKind,
        bytes: &[u8],
    ) -> Result<MainlineArtifact, MainlineRunError> {
        let ref_id = self.next_ref_id();
        let kind_name = mainline_artifact_kind_name(kind);
        let run_dir = self.artifact_root.join(&run_id.0);
        tokio::fs::create_dir_all(&run_dir)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("create artifact dir failed: {err}")))?;
        let path = run_dir.join(format!("{ref_id}-{kind_name}.log"));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("persist artifact failed: {err}")))?;

        Ok(MainlineArtifact {
            ref_id,
            kind,
            path: path.to_string_lossy().to_string(),
            byte_count: bytes.len() as u64,
            line_count: count_newlines(bytes),
        })
    }
}

#[async_trait]
impl MainlineRunner for AppMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        let output = TokioCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .output()
            .await
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;

        let stdout_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stdout,
                &output.stdout,
            )
            .await?;
        let stderr_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stderr,
                &output.stderr,
            )
            .await?;

        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: output.status.code(),
            artifacts: vec![stdout_artifact, stderr_artifact],
        })
    }
}

pub(crate) fn mainline_artifact_kind_name(kind: MainlineArtifactKind) -> &'static str {
    match kind {
        MainlineArtifactKind::Stdout => "stdout",
        MainlineArtifactKind::Stderr => "stderr",
    }
}

pub(super) fn count_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|byte| **byte == b'\n').count() as u64
}
