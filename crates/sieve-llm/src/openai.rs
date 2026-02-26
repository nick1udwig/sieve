use crate::config::{
    ensure_provider_openai, env_getter, load_model_config_from_env, load_openai_api_key_from_env,
};
use crate::wire::{
    decode_planner_output, decode_quarantine_output, extract_openai_message_content_json,
    extract_openai_planner_output_json, planner_regeneration_diagnostic_prompt,
    quarantine_output_schema, serialize_planner_input, PlannerDecodeOutcome, PLANNER_SYSTEM_PROMPT,
    QUARANTINE_SYSTEM_PROMPT,
};
use crate::{LlmError, PlannerModel, QuarantineModel};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sieve_tool_contracts::tool_args_schema;
use sieve_types::{
    LlmModelConfig, PlannerTurnInput, PlannerTurnOutput, QuarantineExtractInput,
    QuarantineExtractOutput, ToolContractValidationReport,
};
use std::future::Future;
use std::time::Duration;

const OPENAI_DEFAULT_API_BASE: &str = "https://api.openai.com";
const HTTP_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_MAX_RETRIES: usize = 2;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 350;

#[derive(Clone)]
struct OpenAiClient {
    http: Client,
    api_key: String,
    api_base: String,
    max_retries: usize,
    retry_backoff: Duration,
}

impl OpenAiClient {
    fn new(api_key: String, api_base: Option<String>) -> Result<Self, LlmError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECONDS))
            .build()
            .map_err(|e| LlmError::Transport(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            http,
            api_key,
            api_base: api_base.unwrap_or_else(|| OPENAI_DEFAULT_API_BASE.to_string()),
            max_retries: DEFAULT_MAX_RETRIES,
            retry_backoff: Duration::from_millis(DEFAULT_RETRY_BACKOFF_MS),
        })
    }

    async fn create_chat_completion(&self, payload: Value) -> Result<Value, LlmError> {
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.api_base.trim_end_matches('/')
        );
        let mut attempt = 0usize;
        loop {
            let request = self
                .http
                .post(&endpoint)
                .bearer_auth(&self.api_key)
                .header("content-type", "application/json")
                .json(&payload);

            match request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.map_err(|e| {
                        LlmError::Transport(format!("failed reading OpenAI response body: {e}"))
                    })?;

                    if status.is_success() {
                        return serde_json::from_str::<Value>(&body).map_err(|e| {
                            LlmError::Decode(format!("invalid OpenAI JSON response: {e}"))
                        });
                    }

                    if is_transient_status(status) && attempt < self.max_retries {
                        attempt += 1;
                        tokio::time::sleep(backoff(self.retry_backoff, attempt)).await;
                        continue;
                    }

                    return Err(LlmError::HttpStatus {
                        status: status.as_u16(),
                        body: truncate_for_error(&body),
                    });
                }
                Err(err) => {
                    let retryable = err.is_timeout() || err.is_connect();
                    if retryable && attempt < self.max_retries {
                        attempt += 1;
                        tokio::time::sleep(backoff(self.retry_backoff, attempt)).await;
                        continue;
                    }
                    if retryable {
                        return Err(LlmError::RetryExhausted(format!(
                            "request failed after retries: {err}"
                        )));
                    }
                    return Err(LlmError::Transport(format!("OpenAI request failed: {err}")));
                }
            }
        }
    }
}

pub struct OpenAiPlannerModel {
    config: LlmModelConfig,
    client: OpenAiClient,
}

impl OpenAiPlannerModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        ensure_provider_openai(&config)?;
        if api_key.trim().is_empty() {
            return Err(LlmError::Config(
                "planner OpenAI API key is empty".to_string(),
            ));
        }
        let client = OpenAiClient::new(api_key, config.api_base.clone())?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_PLANNER", &env_getter)?;
        let api_key = load_openai_api_key_from_env("SIEVE_PLANNER", &env_getter)?;
        Self::new(config, api_key)
    }
}

#[async_trait]
impl PlannerModel for OpenAiPlannerModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        let planner_payload = serialize_planner_input(&input)?;
        let messages = vec![
            json!({"role":"system","content": PLANNER_SYSTEM_PROMPT}),
            json!({"role":"user","content": planner_payload.to_string()}),
        ];

        run_planner_with_one_regeneration(
            self.config.model.as_str(),
            messages,
            &input.allowed_tools,
            |request| self.client.create_chat_completion(request),
        )
        .await
    }
}

pub struct OpenAiQuarantineModel {
    config: LlmModelConfig,
    client: OpenAiClient,
}

impl OpenAiQuarantineModel {
    pub fn new(config: LlmModelConfig, api_key: String) -> Result<Self, LlmError> {
        ensure_provider_openai(&config)?;
        if api_key.trim().is_empty() {
            return Err(LlmError::Config(
                "quarantine OpenAI API key is empty".to_string(),
            ));
        }
        let client = OpenAiClient::new(api_key, config.api_base.clone())?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = load_model_config_from_env("SIEVE_QUARANTINE", &env_getter)?;
        let api_key = load_openai_api_key_from_env("SIEVE_QUARANTINE", &env_getter)?;
        Self::new(config, api_key)
    }
}

#[async_trait]
impl QuarantineModel for OpenAiQuarantineModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn extract_typed(
        &self,
        input: QuarantineExtractInput,
    ) -> Result<QuarantineExtractOutput, LlmError> {
        let request = json!({
            "model": self.config.model,
            "temperature": 0,
            "messages": [
                {"role":"system","content": QUARANTINE_SYSTEM_PROMPT},
                {"role":"user","content": json!({
                    "run_id": input.run_id.0,
                    "prompt": input.prompt,
                    "enum_registry": input.enum_registry
                }).to_string()}
            ],
            "response_format": {
                "type":"json_schema",
                "json_schema": {
                    "name":"quarantine_typed_output",
                    "strict": true,
                    "schema": quarantine_output_schema()
                }
            }
        });

        let response = self.client.create_chat_completion(request).await?;
        let content_json = extract_openai_message_content_json(&response)?;
        decode_quarantine_output(content_json, &input.enum_registry)
    }
}

async fn run_planner_with_one_regeneration<F, Fut>(
    model: &str,
    mut messages: Vec<Value>,
    allowed_tools: &[String],
    mut send_request: F,
) -> Result<PlannerTurnOutput, LlmError>
where
    F: FnMut(Value) -> Fut,
    Fut: Future<Output = Result<Value, LlmError>>,
{
    let mut regenerated = false;

    loop {
        let request = planner_chat_completion_request(model, messages.clone(), allowed_tools)?;
        let response = send_request(request).await?;
        let output_json = extract_openai_planner_output_json(&response)?;

        match decode_planner_output(output_json)? {
            PlannerDecodeOutcome::Valid(output) => {
                ensure_allowed_tools(allowed_tools, &output)?;
                return Ok(output);
            }
            PlannerDecodeOutcome::InvalidToolContracts(report) => {
                if regenerated {
                    return Err(regeneration_exhausted_error(report));
                }
                regenerated = true;
                let prompt = planner_regeneration_diagnostic_prompt(&report)?;
                messages.push(json!({"role":"user","content": prompt}));
            }
        }
    }
}

fn planner_chat_completion_request(
    model: &str,
    messages: Vec<Value>,
    allowed_tools: &[String],
) -> Result<Value, LlmError> {
    let tools = planner_tool_definitions(allowed_tools)?;
    Ok(json!({
        "model": model,
        "temperature": 0,
        "messages": messages,
        "tools": tools,
        "tool_choice": "required"
    }))
}

fn planner_tool_definitions(allowed_tools: &[String]) -> Result<Vec<Value>, LlmError> {
    if allowed_tools.is_empty() {
        return Err(LlmError::Boundary(
            "allowed_tools must include at least one tool".to_string(),
        ));
    }

    let mut tools = Vec::with_capacity(allowed_tools.len());
    for tool_name in allowed_tools {
        let schema = tool_args_schema(tool_name).ok_or_else(|| {
            LlmError::Boundary(format!(
                "allowed tool `{tool_name}` is missing a contract schema"
            ))
        })?;
        tools.push(json!({
            "type": "function",
            "function": {
                "name": tool_name,
                "parameters": schema
            }
        }));
    }

    Ok(tools)
}

fn ensure_allowed_tools(
    allowed_tools: &[String],
    output: &PlannerTurnOutput,
) -> Result<(), LlmError> {
    for call in &output.tool_calls {
        if !allowed_tools
            .iter()
            .any(|allowed| allowed == &call.tool_name)
        {
            return Err(LlmError::Boundary(format!(
                "planner emitted disallowed tool `{}`",
                call.tool_name
            )));
        }
    }
    Ok(())
}

fn regeneration_exhausted_error(report: ToolContractValidationReport) -> LlmError {
    let serialized = serde_json::to_string(&report)
        .unwrap_or_else(|_| "{\"serialization\":\"failed\"}".to_string());
    LlmError::Boundary(format!(
        "planner emitted invalid tool args after one regeneration pass: {serialized}"
    ))
}

fn is_transient_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) || status.is_server_error()
}

fn backoff(base: Duration, attempt: usize) -> Duration {
    let shift = (attempt.saturating_sub(1)) as u32;
    let multiplier = 1u32.checked_shl(shift).unwrap_or(u32::MAX);
    base.saturating_mul(multiplier)
}

fn truncate_for_error(input: &str) -> String {
    const MAX: usize = 512;
    if input.len() <= MAX {
        input.to_string()
    } else {
        format!("{}...[truncated]", &input[..MAX])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn planner_native_tool_response(tool_calls: Value) -> Value {
        json!({
            "choices": [
                {
                    "message": {
                        "content": null,
                        "tool_calls": tool_calls
                    }
                }
            ]
        })
    }

    #[tokio::test]
    async fn planner_request_includes_openai_native_tools_payload() {
        let responses = Arc::new(Mutex::new(VecDeque::from(vec![
            planner_native_tool_response(json!([
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":\"ls -la\"}"
                    }
                }
            ])),
        ])));
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));

        let _ = run_planner_with_one_regeneration(
            "gpt-test",
            vec![
                json!({"role":"system","content":"sys"}),
                json!({"role":"user","content":"input"}),
            ],
            &["bash".to_string()],
            {
                let responses = responses.clone();
                let requests = requests.clone();
                move |request| {
                    requests.lock().expect("request lock").push(request);
                    let response = responses
                        .lock()
                        .expect("response lock")
                        .pop_front()
                        .expect("mock response");
                    async move { Ok(response) }
                }
            },
        )
        .await
        .expect("planner request should succeed");

        let requests = requests.lock().expect("request lock");
        assert!(requests[0].pointer("/tools").is_some());
    }

    #[tokio::test]
    async fn planner_accepts_openai_native_tool_call_response_shape() {
        let responses = Arc::new(Mutex::new(VecDeque::from(vec![
            planner_native_tool_response(json!([
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":\"ls -la\"}"
                    }
                }
            ])),
        ])));

        let output = run_planner_with_one_regeneration(
            "gpt-test",
            vec![
                json!({"role":"system","content":"sys"}),
                json!({"role":"user","content":"input"}),
            ],
            &["bash".to_string()],
            {
                let responses = responses.clone();
                move |_request| {
                    let response = responses
                        .lock()
                        .expect("response lock")
                        .pop_front()
                        .expect("mock response");
                    async move { Ok(response) }
                }
            },
        )
        .await
        .expect("native tool-calls should decode");

        assert_eq!(output.tool_calls.len(), 1);
        assert_eq!(output.tool_calls[0].tool_name, "bash");
        assert_eq!(output.tool_calls[0].args.get("cmd"), Some(&json!("ls -la")));
    }

    #[tokio::test]
    async fn planner_regenerates_once_then_succeeds() {
        let responses = Arc::new(Mutex::new(VecDeque::from(vec![
            planner_native_tool_response(json!([
                {
                    "id": "call_invalid",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":123}"
                    }
                }
            ])),
            planner_native_tool_response(json!([
                {
                    "id": "call_valid",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":\"ls -la\"}"
                    }
                }
            ])),
        ])));
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));

        let output = run_planner_with_one_regeneration(
            "gpt-test",
            vec![
                json!({"role":"system","content":"sys"}),
                json!({"role":"user","content":"input"}),
            ],
            &["bash".to_string()],
            {
                let responses = responses.clone();
                let requests = requests.clone();
                move |request| {
                    requests.lock().expect("request lock").push(request);
                    let response = responses
                        .lock()
                        .expect("response lock")
                        .pop_front()
                        .expect("mock response");
                    async move { Ok(response) }
                }
            },
        )
        .await
        .expect("regeneration should recover");

        assert_eq!(output.tool_calls.len(), 1);
        assert_eq!(output.tool_calls[0].tool_name, "bash");
        assert_eq!(output.tool_calls[0].args.get("cmd"), Some(&json!("ls -la")));

        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2);
        let retry_prompt = requests[1]
            .pointer("/messages/2/content")
            .and_then(Value::as_str)
            .expect("retry diagnostic prompt");
        assert!(retry_prompt.contains("tool_call_index"));
        assert!(retry_prompt.contains("Diagnostics"));
    }

    #[tokio::test]
    async fn planner_fails_after_one_regeneration_pass() {
        let responses = Arc::new(Mutex::new(VecDeque::from(vec![
            planner_native_tool_response(json!([
                {
                    "id": "call_invalid_1",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":123}"
                    }
                }
            ])),
            planner_native_tool_response(json!([
                {
                    "id": "call_invalid_2",
                    "type": "function",
                    "function": {
                        "name": "bash",
                        "arguments": "{\"cmd\":456}"
                    }
                }
            ])),
        ])));

        let err = run_planner_with_one_regeneration(
            "gpt-test",
            vec![
                json!({"role":"system","content":"sys"}),
                json!({"role":"user","content":"input"}),
            ],
            &["bash".to_string()],
            {
                let responses = responses.clone();
                move |_request| {
                    let response = responses
                        .lock()
                        .expect("response lock")
                        .pop_front()
                        .expect("mock response");
                    async move { Ok(response) }
                }
            },
        )
        .await
        .expect_err("second validation failure should hard-fail");

        match err {
            LlmError::Boundary(message) => {
                assert!(message.contains("after one regeneration pass"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
