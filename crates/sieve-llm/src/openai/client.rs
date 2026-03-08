use super::exchange_logger::LlmExchangeLogger;
use super::planner_retry::{backoff, is_transient_status, truncate_for_error};
use crate::LlmError;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const OPENAI_DEFAULT_API_BASE: &str = "https://api.openai.com";
const HTTP_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_MAX_RETRIES: usize = 2;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 350;

#[derive(Clone)]
pub(super) struct OpenAiClient {
    http: Client,
    api_key: String,
    api_base: String,
    max_retries: usize,
    retry_backoff: Duration,
    exchange_logger: LlmExchangeLogger,
}

impl OpenAiClient {
    pub(super) fn new(api_key: String, api_base: Option<String>) -> Result<Self, LlmError> {
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
            exchange_logger: LlmExchangeLogger::from_env("openai"),
        })
    }

    pub(super) async fn create_chat_completion(&self, payload: Value) -> Result<Value, LlmError> {
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.api_base.trim_end_matches('/')
        );
        let mut attempt = 0usize;
        loop {
            let attempt_number = attempt + 1;
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
                    self.exchange_logger.log_http(
                        &endpoint,
                        &payload,
                        attempt_number,
                        status.as_u16(),
                        &body,
                    );

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
                    self.exchange_logger.log_transport_error(
                        &endpoint,
                        &payload,
                        attempt_number,
                        &err.to_string(),
                    );
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
