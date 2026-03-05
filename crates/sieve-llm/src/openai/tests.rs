use super::*;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("sieve-llm-{name}-{nanos}.jsonl"))
}

#[test]
fn exchange_logger_writes_http_event_with_exact_payloads() {
    let path = unique_temp_path("exchange-http");
    let logger = LlmExchangeLogger::with_path(Some(path.clone()));
    let request_json = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role":"user","content":"hi"}]
    });
    let response_body = "{\"id\":\"resp_1\",\"choices\":[]}";

    logger.log_http(
        "https://api.openai.com/v1/chat/completions",
        &request_json,
        1,
        200,
        response_body,
    );

    let body = fs::read_to_string(&path).expect("read exchange log");
    let line = body.lines().next().expect("one log line");
    let record: Value = serde_json::from_str(line).expect("parse record json");
    assert_eq!(record["event"], "llm_provider_exchange");
    assert_eq!(
        record["endpoint"],
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(record["attempt"], 1);
    assert_eq!(record["response_status"], 200);
    assert_eq!(record["request_json"], request_json);
    assert_eq!(record["response_body"], response_body);

    let _ = fs::remove_file(path);
}

#[test]
fn exchange_logger_writes_transport_error_event() {
    let path = unique_temp_path("exchange-transport");
    let logger = LlmExchangeLogger::with_path(Some(path.clone()));
    let request_json = json!({
        "model": "gpt-4o-mini",
        "messages": [{"role":"user","content":"hello"}]
    });

    logger.log_transport_error(
        "https://api.openai.com/v1/chat/completions",
        &request_json,
        2,
        "connection reset",
    );

    let body = fs::read_to_string(&path).expect("read exchange log");
    let line = body.lines().next().expect("one log line");
    let record: Value = serde_json::from_str(line).expect("parse record json");
    assert_eq!(record["event"], "llm_provider_exchange");
    assert_eq!(record["attempt"], 2);
    assert_eq!(record["request_json"], request_json);
    assert_eq!(record["transport_error"], "connection reset");
    assert!(record.get("response_body").is_none());

    let _ = fs::remove_file(path);
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
    assert_eq!(
        requests[0]
            .pointer("/tool_choice")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "auto"
    );
}

#[tokio::test]
async fn planner_request_without_allowed_tools_omits_tools_payload() {
    let responses = Arc::new(Mutex::new(VecDeque::from(vec![json!({
        "choices": [
            {
                "message": {
                    "content": "",
                    "tool_calls": []
                }
            }
        ]
    })])));
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));

    let _ = run_planner_with_one_regeneration(
        "gpt-test",
        vec![
            json!({"role":"system","content":"sys"}),
            json!({"role":"user","content":"input"}),
        ],
        &[],
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
    assert!(requests[0].get("tools").is_none());
    assert!(requests[0].get("tool_choice").is_none());
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
