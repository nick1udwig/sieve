#![forbid(unsafe_code)]

mod app_server;
mod openai;

use crate::error::CapTraceError;
use async_trait::async_trait;
use std::sync::Arc;

use app_server::CodexAppServerCaseGenerator;
pub use openai::PlannerCaseGenerator;

const DEFAULT_CODEX_APP_SERVER_WS_URL: &str = "ws://127.0.0.1:4500";
const DEFAULT_CODEX_MODEL: &str = "gpt-5.2-codex";
const DEFAULT_CODEX_CONNECT_TIMEOUT_MS: u64 = 500;
const DEFAULT_CODEX_TURN_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct CaseGenerationRequest {
    pub command: String,
    pub max_cases: usize,
}

#[async_trait]
pub trait CaseGenerator: Send + Sync {
    async fn generate_cases(
        &self,
        request: CaseGenerationRequest,
    ) -> Result<Vec<Vec<String>>, CapTraceError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseGeneratorBackend {
    CodexAppServer,
    OpenAiPlanner,
}

impl CaseGeneratorBackend {
    pub fn name(self) -> &'static str {
        match self {
            CaseGeneratorBackend::CodexAppServer => "codex-app-server",
            CaseGeneratorBackend::OpenAiPlanner => "openai-planner",
        }
    }
}

pub async fn preferred_case_generator_from_env(
) -> Result<(Arc<dyn CaseGenerator>, CaseGeneratorBackend), CapTraceError> {
    let app_server = CodexAppServerCaseGenerator::from_env();
    if app_server.is_running().await {
        return Ok((Arc::new(app_server), CaseGeneratorBackend::CodexAppServer));
    }

    let planner = PlannerCaseGenerator::from_env()?;
    Ok((Arc::new(planner), CaseGeneratorBackend::OpenAiPlanner))
}

pub(super) fn generation_prompt(command: &str, max_cases: usize) -> String {
    format!(
        "Return JSON only with shape {{\"cases\": [string...]}}. Generate up to {max_cases} shell command strings. Each command must invoke `{command}` only. No pipes, no control operators, no shell variables. Use placeholders {{TMP_DIR}} {{IN_FILE}} {{IN_FILE_2}} {{OUT_FILE}} {{URL}} {{HEADER}} {{DATA}} {{KV}} {{ARG}}. Focus on valid command usage that should run successfully. Explore different subcommands and meaningful flag combinations. Avoid help/version and flags likely to be unsupported by the command unless no other runnable forms exist."
    )
}

pub(super) fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(super) fn parse_u64_env(key: &str, default: u64) -> u64 {
    env_non_empty(key)
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(default)
}
