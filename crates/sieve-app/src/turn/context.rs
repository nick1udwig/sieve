use crate::config::AppConfig;
use crate::lcm_integration::LcmIntegration;
use crate::logging::FanoutRuntimeEventLog;
use sieve_llm::{GuidanceModel, ResponseModel, SummaryModel};
use sieve_runtime::RuntimeOrchestrator;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct TurnDeps {
    pub(crate) runtime: Arc<RuntimeOrchestrator>,
    pub(crate) guidance_model: Arc<dyn GuidanceModel>,
    pub(crate) response_model: Arc<dyn ResponseModel>,
    pub(crate) summary_model: Arc<dyn SummaryModel>,
    pub(crate) lcm: Option<Arc<LcmIntegration>>,
    pub(crate) event_log: Arc<FanoutRuntimeEventLog>,
    pub(crate) cfg: AppConfig,
}
