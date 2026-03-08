use super::requests::resolve_codex_url;
use crate::auth::OpenAiCodexAuth;
use crate::openai::LlmExchangeLogger;
use crate::openai::{backoff, is_transient_status, truncate_for_error};
use crate::LlmError;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const HTTP_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_MAX_RETRIES: usize = 2;
const DEFAULT_RETRY_BACKOFF_MS: u64 = 350;

#[derive(Clone)]
pub(crate) struct OpenAiCodexClient {
    http: Client,
    auth: OpenAiCodexAuth,
    api_base: Option<String>,
    max_retries: usize,
    retry_backoff: Duration,
    exchange_logger: LlmExchangeLogger,
}

impl OpenAiCodexClient {
    pub(crate) fn new(auth: OpenAiCodexAuth, api_base: Option<String>) -> Result<Self, LlmError> {
        let http = Client::builder()
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECONDS))
            .build()
            .map_err(|e| LlmError::Transport(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            http,
            auth,
            api_base,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_backoff: Duration::from_millis(DEFAULT_RETRY_BACKOFF_MS),
            exchange_logger: LlmExchangeLogger::from_env("openai_codex"),
        })
    }

    pub(crate) async fn create_response(&self, payload: Value) -> Result<Value, LlmError> {
        let endpoint = resolve_codex_url(self.api_base.as_deref());
        let mut attempt = 0usize;

        loop {
            let attempt_number = attempt + 1;
            let (access_token, account_id) =
                self.auth.access_token_and_account_id(&self.http).await?;

            let request = self
                .http
                .post(&endpoint)
                .bearer_auth(&access_token)
                .header("chatgpt-account-id", account_id)
                .header("OpenAI-Beta", "responses=experimental")
                .header("originator", "sieve")
                .header("accept", "text/event-stream")
                .header("content-type", "application/json")
                .json(&payload);

            match request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.map_err(|e| {
                        LlmError::Transport(format!(
                            "failed reading openai_codex response body: {e}"
                        ))
                    })?;
                    self.exchange_logger.log_http(
                        &endpoint,
                        &payload,
                        attempt_number,
                        status.as_u16(),
                        &body,
                    );

                    if status.is_success() {
                        return parse_codex_response_body(&body);
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
                    return Err(LlmError::Transport(format!(
                        "openai_codex request failed: {err}"
                    )));
                }
            }
        }
    }
}

fn parse_codex_response_body(body: &str) -> Result<Value, LlmError> {
    if body.trim_start().starts_with("event:") {
        return parse_codex_event_stream(body);
    }
    serde_json::from_str::<Value>(body)
        .map_err(|e| LlmError::Decode(format!("invalid openai_codex JSON response: {e}")))
}

fn parse_codex_event_stream(body: &str) -> Result<Value, LlmError> {
    let mut current_event = None::<String>;
    let mut current_data = Vec::<String>::new();
    let mut completed_response = None::<Value>;
    let mut failed_error = None::<String>;

    let flush_event = |event: &mut Option<String>,
                       data: &mut Vec<String>,
                       completed_response: &mut Option<Value>,
                       failed_error: &mut Option<String>|
     -> Result<(), LlmError> {
        let Some(event_name) = event.take() else {
            data.clear();
            return Ok(());
        };
        if data.is_empty() {
            return Ok(());
        }
        let payload = data.join("\n");
        data.clear();
        let parsed = serde_json::from_str::<Value>(&payload).map_err(|e| {
            LlmError::Decode(format!(
                "invalid openai_codex SSE event `{event_name}` JSON: {e}"
            ))
        })?;
        match event_name.as_str() {
            "response.completed" => {
                *completed_response = Some(parsed.get("response").cloned().ok_or_else(|| {
                    LlmError::Decode(
                        "openai_codex SSE completed event missing `response`".to_string(),
                    )
                })?);
            }
            "response.failed" => {
                let error = parsed
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .or_else(|| parsed.pointer("/response/error").and_then(Value::as_str))
                    .or_else(|| parsed.get("error").and_then(Value::as_str))
                    .unwrap_or("response.failed");
                *failed_error = Some(error.to_string());
            }
            _ => {}
        }
        Ok(())
    };

    for line in body.lines() {
        if line.is_empty() {
            flush_event(
                &mut current_event,
                &mut current_data,
                &mut completed_response,
                &mut failed_error,
            )?;
            continue;
        }
        if let Some(event_name) = line.strip_prefix("event:") {
            current_event = Some(event_name.trim().to_string());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            current_data.push(data.trim_start().to_string());
        }
    }
    flush_event(
        &mut current_event,
        &mut current_data,
        &mut completed_response,
        &mut failed_error,
    )?;

    if let Some(response) = completed_response {
        return Ok(response);
    }
    if let Some(error) = failed_error {
        return Err(LlmError::Backend(format!(
            "openai_codex streamed response failed: {error}"
        )));
    }
    Err(LlmError::Decode(
        "openai_codex SSE response missing `response.completed` event".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{parse_codex_event_stream, parse_codex_response_body};

    #[test]
    fn parses_streamed_completed_message_response() {
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"output\":[]}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"{\\\"ok\\\":true}\"}]}]}}\n\n"
        );
        let parsed = parse_codex_event_stream(body).expect("parse stream");
        assert_eq!(parsed.get("id").and_then(|v| v.as_str()), Some("resp_1"));
        assert_eq!(
            parsed
                .pointer("/output/0/content/0/text")
                .and_then(|v| v.as_str()),
            Some("{\"ok\":true}")
        );
    }

    #[test]
    fn parses_streamed_completed_function_call_response() {
        let body = concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_2\",\"output\":[{\"type\":\"function_call\",\"name\":\"ping\",\"arguments\":\"{\\\"x\\\":1}\"}]}}\n\n"
        );
        let parsed = parse_codex_response_body(body).expect("parse stream body");
        assert_eq!(parsed.get("id").and_then(|v| v.as_str()), Some("resp_2"));
        assert_eq!(
            parsed.pointer("/output/0/name").and_then(|v| v.as_str()),
            Some("ping")
        );
        assert_eq!(
            parsed
                .pointer("/output/0/arguments")
                .and_then(|v| v.as_str()),
            Some("{\"x\":1}")
        );
    }

    #[test]
    fn errors_when_stream_missing_completed_event() {
        let err = parse_codex_event_stream(
            "event: response.in_progress\ndata: {\"type\":\"response.in_progress\"}\n\n",
        )
        .expect_err("missing completed");
        assert!(err
            .to_string()
            .contains("openai_codex SSE response missing `response.completed` event"));
    }
}
