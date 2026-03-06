use super::*;
pub(crate) fn env_test_lock() -> &'static StdMutex<()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
}

pub(crate) struct EchoSummaryModel;

#[async_trait]
impl SummaryModel for EchoSummaryModel {
    fn config(&self) -> &sieve_types::LlmModelConfig {
        static CONFIG: OnceLock<sieve_types::LlmModelConfig> = OnceLock::new();
        CONFIG.get_or_init(|| sieve_types::LlmModelConfig {
            provider: sieve_types::LlmProvider::OpenAi,
            model: "summary-test".to_string(),
            api_base: None,
        })
    }

    async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError> {
        if request.ref_id.starts_with("assistant-compose-quality:") {
            return Ok("PASS".to_string());
        }
        if request.ref_id.starts_with("assistant-compose-grounding:") {
            return Ok("PASS".to_string());
        }
        if request.ref_id.starts_with("assistant-compose-gate:") {
            return Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":null}".to_string());
        }
        if request.ref_id.starts_with("assistant-compose:")
            || request.ref_id.starts_with("assistant-compose-retry:")
            || request
                .ref_id
                .starts_with("assistant-compose-grounded-retry:")
        {
            if let Ok(payload) = serde_json::from_str::<Value>(&request.content) {
                if let Some(draft) = payload
                    .get("assistant_draft_message")
                    .and_then(Value::as_str)
                {
                    return Ok(draft.to_string());
                }
                if let Some(previous) = payload
                    .get("previous_composed_message")
                    .and_then(Value::as_str)
                {
                    return Ok(previous.to_string());
                }
            }
            return Ok(String::new());
        }
        Ok(format!(
            "summary(bytes={},lines={})",
            request.byte_count, request.line_count
        ))
    }
}

pub(crate) struct PassThroughSummaryModel;

#[async_trait]
impl SummaryModel for PassThroughSummaryModel {
    fn config(&self) -> &sieve_types::LlmModelConfig {
        static CONFIG: OnceLock<sieve_types::LlmModelConfig> = OnceLock::new();
        CONFIG.get_or_init(|| sieve_types::LlmModelConfig {
            provider: sieve_types::LlmProvider::OpenAi,
            model: "summary-pass-through-test".to_string(),
            api_base: None,
        })
    }

    async fn summarize_ref(&self, request: SummaryRequest) -> Result<String, LlmError> {
        if request.ref_id.starts_with("assistant-compose-quality:") {
            return Ok("PASS".to_string());
        }
        if request.ref_id.starts_with("assistant-compose-grounding:") {
            return Ok("PASS".to_string());
        }
        if request.ref_id.starts_with("assistant-compose-gate:") {
            return Ok("{\"verdict\":\"PASS\",\"reason\":\"\",\"continue_code\":null}".to_string());
        }
        if request.ref_id.starts_with("assistant-compose:")
            || request.ref_id.starts_with("assistant-compose-retry:")
            || request
                .ref_id
                .starts_with("assistant-compose-grounded-retry:")
        {
            if let Ok(payload) = serde_json::from_str::<Value>(&request.content) {
                if let Some(draft) = payload
                    .get("assistant_draft_message")
                    .and_then(Value::as_str)
                {
                    return Ok(draft.to_string());
                }
                if let Some(previous) = payload
                    .get("previous_composed_message")
                    .and_then(Value::as_str)
                {
                    return Ok(previous.to_string());
                }
            }
            return Ok(String::new());
        }
        Ok(request.content)
    }
}

pub(crate) struct QueuedSummaryModel {
    config: LlmModelConfig,
    outputs: StdMutex<VecDeque<Result<String, LlmError>>>,
    calls: AtomicU64,
}

impl QueuedSummaryModel {
    pub(crate) fn new(outputs: Vec<Result<String, LlmError>>) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "summary-queue-test".to_string(),
                api_base: None,
            },
            outputs: StdMutex::new(VecDeque::from(outputs)),
            calls: AtomicU64::new(0),
        }
    }

    pub(crate) fn call_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl SummaryModel for QueuedSummaryModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn summarize_ref(&self, _request: SummaryRequest) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.outputs
            .lock()
            .expect("summary queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(|| {
                Err(LlmError::Backend(
                    "summary queue exhausted with no configured output".to_string(),
                ))
            })
    }
}

pub(crate) const E2E_POLICY_BASE: &str = r#"
[options]
violation_mode = "ask"
trusted_control = true
require_trusted_control_for_mutating = true

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://api.search.brave.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://markdown.new/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://weather.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.weather.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://forecast.weather.gov/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://wunderground.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.wunderground.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://google.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.google.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://x.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://www.x.com/"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://api.open-meteo.com/"
"#;

pub(crate) struct QueuedPlannerModel {
    config: LlmModelConfig,
    outputs: StdMutex<VecDeque<Result<PlannerTurnOutput, LlmError>>>,
    calls: AtomicU64,
}

impl QueuedPlannerModel {
    pub(crate) fn new(outputs: Vec<Result<PlannerTurnOutput, LlmError>>) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "planner-test".to_string(),
                api_base: None,
            },
            outputs: StdMutex::new(VecDeque::from(outputs)),
            calls: AtomicU64::new(0),
        }
    }

    pub(crate) fn call_count(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl PlannerModel for QueuedPlannerModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, _input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.outputs
            .lock()
            .expect("planner queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(|| {
                Err(LlmError::Backend(
                    "planner queue exhausted with no configured output".to_string(),
                ))
            })
    }
}

pub(crate) struct QueuedGuidanceModel {
    config: LlmModelConfig,
    outputs: StdMutex<VecDeque<Result<PlannerGuidanceOutput, LlmError>>>,
}

impl QueuedGuidanceModel {
    pub(crate) fn new(outputs: Vec<Result<PlannerGuidanceOutput, LlmError>>) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "guidance-test".to_string(),
                api_base: None,
            },
            outputs: StdMutex::new(VecDeque::from(outputs)),
        }
    }
}

#[async_trait]
impl GuidanceModel for QueuedGuidanceModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn classify_guidance(
        &self,
        _input: PlannerGuidanceInput,
    ) -> Result<PlannerGuidanceOutput, LlmError> {
        self.outputs
            .lock()
            .expect("guidance queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(|| {
                Ok(PlannerGuidanceOutput {
                    guidance: PlannerGuidanceFrame {
                        code: PlannerGuidanceSignal::FinalAnswerReady.code(),
                        confidence_bps: 10_000,
                        source_hit_index: None,
                        evidence_ref_index: None,
                    },
                })
            })
    }
}

pub(crate) fn guidance_output(signal: PlannerGuidanceSignal) -> PlannerGuidanceOutput {
    PlannerGuidanceOutput {
        guidance: PlannerGuidanceFrame {
            code: signal.code(),
            confidence_bps: 10_000,
            source_hit_index: None,
            evidence_ref_index: None,
        },
    }
}

pub(crate) struct QueuedResponseModel {
    config: LlmModelConfig,
    outputs: StdMutex<VecDeque<Result<sieve_llm::ResponseTurnOutput, LlmError>>>,
}

impl QueuedResponseModel {
    pub(crate) fn new(outputs: Vec<Result<sieve_llm::ResponseTurnOutput, LlmError>>) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "response-test".to_string(),
                api_base: None,
            },
            outputs: StdMutex::new(VecDeque::from(outputs)),
        }
    }
}

#[async_trait]
impl ResponseModel for QueuedResponseModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn write_turn_response(
        &self,
        _input: ResponseTurnInput,
    ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
        self.outputs
            .lock()
            .expect("response queue mutex poisoned")
            .pop_front()
            .unwrap_or_else(|| {
                Err(LlmError::Backend(
                    "response queue exhausted with no configured output".to_string(),
                ))
            })
    }
}

pub(crate) struct RecordingResponseModel {
    config: LlmModelConfig,
    output: sieve_llm::ResponseTurnOutput,
    last_input: StdMutex<Option<ResponseTurnInput>>,
}

impl RecordingResponseModel {
    pub(crate) fn new(message: &str) -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "response-recording-test".to_string(),
                api_base: None,
            },
            output: sieve_llm::ResponseTurnOutput {
                message: message.to_string(),
                referenced_ref_ids: BTreeSet::new(),
                summarized_ref_ids: BTreeSet::new(),
            },
            last_input: StdMutex::new(None),
        }
    }

    pub(crate) fn last_input(&self) -> Option<ResponseTurnInput> {
        self.last_input
            .lock()
            .expect("recording response mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl ResponseModel for RecordingResponseModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn write_turn_response(
        &self,
        input: ResponseTurnInput,
    ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
        self.last_input
            .lock()
            .expect("recording response mutex poisoned")
            .replace(input);
        Ok(self.output.clone())
    }
}

pub(crate) struct FirstStdoutSummaryResponseModel {
    config: LlmModelConfig,
}

impl FirstStdoutSummaryResponseModel {
    pub(crate) fn new() -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "response-first-stdout-test".to_string(),
                api_base: None,
            },
        }
    }
}

#[async_trait]
impl ResponseModel for FirstStdoutSummaryResponseModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn write_turn_response(
        &self,
        input: ResponseTurnInput,
    ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
        let stdout_ref = input
            .tool_outcomes
            .iter()
            .flat_map(|outcome| outcome.refs.iter())
            .find(|metadata| metadata.kind == "stdout" && metadata.byte_count > 0)
            .map(|metadata| metadata.ref_id.clone())
            .ok_or_else(|| {
                LlmError::Backend("missing stdout ref for response rendering".to_string())
            })?;
        Ok(sieve_llm::ResponseTurnOutput {
            message: format!("[[summary:{stdout_ref}]]"),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::from([stdout_ref]),
        })
    }
}

pub(crate) struct MemoryRecallResponseModel {
    config: LlmModelConfig,
}

impl MemoryRecallResponseModel {
    pub(crate) fn new() -> Self {
        Self {
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "response-memory-recall-test".to_string(),
                api_base: None,
            },
        }
    }
}

#[async_trait]
impl ResponseModel for MemoryRecallResponseModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn write_turn_response(
        &self,
        input: ResponseTurnInput,
    ) -> Result<sieve_llm::ResponseTurnOutput, LlmError> {
        let lower_prompt = input.trusted_user_message.to_ascii_lowercase();
        let lower_thoughts = input
            .planner_thoughts
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let message = if lower_prompt.contains("where do i live") {
            if lower_thoughts.contains("hi i live in livermore ca") {
                "You live in Livermore ca.".to_string()
            } else {
                "I don't know where you live.".to_string()
            }
        } else {
            "Thanks for sharing.".to_string()
        };

        Ok(sieve_llm::ResponseTurnOutput {
            message,
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        })
    }
}

pub(crate) enum E2eModelMode {
    Fake {
        planner: Arc<dyn PlannerModel>,
        guidance: Arc<dyn GuidanceModel>,
        response: Arc<dyn ResponseModel>,
        summary: Arc<dyn SummaryModel>,
    },
    RealOpenAi,
}
