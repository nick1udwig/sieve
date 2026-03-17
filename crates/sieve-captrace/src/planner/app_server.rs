use super::{
    env_non_empty, generation_prompt, parse_u64_env, CaseGenerationRequest, CaseGenerator,
    DEFAULT_CODEX_APP_SERVER_WS_URL, DEFAULT_CODEX_CONNECT_TIMEOUT_MS, DEFAULT_CODEX_MODEL,
    DEFAULT_CODEX_TURN_TIMEOUT_MS,
};
use crate::command_match::argv_matches_command;
use crate::error::CapTraceError;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use std::collections::{BTreeSet, VecDeque};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration, Instant};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

pub(super) struct CodexAppServerCaseGenerator {
    ws_url: String,
    model: String,
    connect_timeout: Duration,
    turn_timeout: Duration,
    shell: BasicShellAnalyzer,
}

impl CodexAppServerCaseGenerator {
    pub(super) fn from_env() -> Self {
        Self {
            ws_url: env_non_empty("SIEVE_CODEX_APP_SERVER_WS_URL")
                .unwrap_or_else(|| DEFAULT_CODEX_APP_SERVER_WS_URL.to_string()),
            model: env_non_empty("SIEVE_CODEX_MODEL")
                .unwrap_or_else(|| DEFAULT_CODEX_MODEL.to_string()),
            connect_timeout: Duration::from_millis(parse_u64_env(
                "SIEVE_CODEX_APP_SERVER_CONNECT_TIMEOUT_MS",
                DEFAULT_CODEX_CONNECT_TIMEOUT_MS,
            )),
            turn_timeout: Duration::from_millis(parse_u64_env(
                "SIEVE_CODEX_APP_SERVER_TURN_TIMEOUT_MS",
                DEFAULT_CODEX_TURN_TIMEOUT_MS,
            )),
            shell: BasicShellAnalyzer,
        }
    }

    pub(super) async fn is_running(&self) -> bool {
        let Ok(mut client) = AppServerWsClient::connect(&self.ws_url, self.connect_timeout).await
        else {
            return false;
        };
        client.initialize().await.is_ok()
    }
}

#[async_trait]
impl CaseGenerator for CodexAppServerCaseGenerator {
    async fn generate_cases(
        &self,
        request: CaseGenerationRequest,
    ) -> Result<Vec<Vec<String>>, CapTraceError> {
        let mut client = AppServerWsClient::connect(&self.ws_url, self.connect_timeout).await?;
        client.initialize().await?;
        let thread_id = client.start_thread(&self.model).await?;
        let user_message = generation_prompt(&request.command, request.max_cases);
        let turn_id = client.start_turn(&thread_id, &user_message).await?;
        let raw = client
            .wait_for_turn_agent_message(&turn_id, self.turn_timeout)
            .await?;
        let commands = parse_cases_from_agent_message(&raw)?;

        let mut unique = BTreeSet::new();
        let mut cases = Vec::new();
        for cmd in commands {
            let analysis = self
                .shell
                .analyze_shell_lc_script(&cmd)
                .map_err(|err| CapTraceError::Shell(err.to_string()))?;
            if analysis.knowledge != sieve_types::CommandKnowledge::Known
                || analysis.segments.len() != 1
            {
                continue;
            }
            let argv = analysis.segments[0].argv.clone();
            if !argv_matches_command(&argv, &request.command) {
                continue;
            }
            let key = argv.join("\u{1f}");
            if unique.insert(key) {
                cases.push(argv);
            }
            if cases.len() >= request.max_cases {
                break;
            }
        }

        if cases.is_empty() {
            return Err(CapTraceError::Llm(
                "codex app-server returned no valid command cases".to_string(),
            ));
        }
        Ok(cases)
    }
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct AppServerWsClient {
    socket: WsStream,
    pending: VecDeque<Value>,
    next_id: u64,
}

#[derive(Serialize)]
struct InitializeParams<'a> {
    #[serde(rename = "clientInfo")]
    client_info: ClientInfo<'a>,
}

#[derive(Serialize)]
struct ClientInfo<'a> {
    name: &'a str,
    title: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct ThreadStartParams<'a> {
    model: &'a str,
}

#[derive(Serialize)]
struct TurnStartParams<'a> {
    #[serde(rename = "threadId")]
    thread_id: &'a str,
    input: [TurnInput<'a>; 1],
    #[serde(rename = "outputSchema")]
    output_schema: TurnOutputSchema,
}

#[derive(Serialize)]
struct TurnInput<'a> {
    #[serde(rename = "type")]
    input_type: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
struct TurnOutputSchema {
    #[serde(rename = "type")]
    schema_type: &'static str,
    required: [&'static str; 1],
    #[serde(rename = "additionalProperties")]
    additional_properties: bool,
    properties: TurnOutputProperties,
}

#[derive(Serialize)]
struct TurnOutputProperties {
    cases: TurnCasesProperty,
}

#[derive(Serialize)]
struct TurnCasesProperty {
    #[serde(rename = "type")]
    property_type: &'static str,
    items: TurnCaseItem,
    #[serde(rename = "minItems")]
    min_items: u8,
}

#[derive(Serialize)]
struct TurnCaseItem {
    #[serde(rename = "type")]
    item_type: &'static str,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a, T: Serialize> {
    method: &'a str,
    id: u64,
    params: T,
}

#[derive(Serialize)]
struct JsonRpcNotification<'a, T: Serialize> {
    method: &'a str,
    params: T,
}

impl AppServerWsClient {
    async fn connect(ws_url: &str, connect_timeout: Duration) -> Result<Self, CapTraceError> {
        let (socket, _) = timeout(connect_timeout, connect_async(ws_url))
            .await
            .map_err(|_| {
                CapTraceError::Llm(format!("codex app-server connect timeout at {ws_url}"))
            })?
            .map_err(|err| {
                CapTraceError::Llm(format!("codex app-server not reachable at {ws_url}: {err}"))
            })?;
        Ok(Self {
            socket,
            pending: VecDeque::new(),
            next_id: 1,
        })
    }

    async fn initialize(&mut self) -> Result<(), CapTraceError> {
        self.request(
            "initialize",
            to_json_value(
                InitializeParams {
                    client_info: ClientInfo {
                        name: "sieve_captrace",
                        title: "Sieve CapTrace",
                        version: env!("CARGO_PKG_VERSION"),
                    },
                },
                "captrace initialize params",
            ),
            Duration::from_secs(5),
        )
        .await?;
        self.notify("initialized", Value::Object(Default::default()))
            .await?;
        Ok(())
    }

    async fn start_thread(&mut self, model: &str) -> Result<String, CapTraceError> {
        let result = self
            .request(
                "thread/start",
                to_json_value(ThreadStartParams { model }, "captrace thread start params"),
                Duration::from_secs(10),
            )
            .await?;
        result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                CapTraceError::Llm("missing thread id from codex app-server".to_string())
            })
    }

    async fn start_turn(&mut self, thread_id: &str, prompt: &str) -> Result<String, CapTraceError> {
        let result = self
            .request(
                "turn/start",
                to_json_value(
                    TurnStartParams {
                        thread_id,
                        input: [TurnInput {
                            input_type: "text",
                            text: prompt,
                        }],
                        output_schema: TurnOutputSchema {
                            schema_type: "object",
                            required: ["cases"],
                            additional_properties: false,
                            properties: TurnOutputProperties {
                                cases: TurnCasesProperty {
                                    property_type: "array",
                                    items: TurnCaseItem {
                                        item_type: "string",
                                    },
                                    min_items: 1,
                                },
                            },
                        },
                    },
                    "captrace turn start params",
                ),
                Duration::from_secs(15),
            )
            .await?;

        result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| CapTraceError::Llm("missing turn id from codex app-server".to_string()))
    }

    async fn wait_for_turn_agent_message(
        &mut self,
        turn_id: &str,
        max_wait: Duration,
    ) -> Result<String, CapTraceError> {
        let deadline = Instant::now() + max_wait;
        let mut delta = String::new();
        let mut completed_text: Option<String> = None;

        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(CapTraceError::Llm(format!(
                    "timeout waiting for codex app-server turn completion for {turn_id}"
                )));
            }
            let remaining = deadline.saturating_duration_since(now);
            let message = self.next_message(remaining).await?;
            let Some(method) = message.get("method").and_then(Value::as_str) else {
                continue;
            };

            match method {
                "item/agentMessage/delta" => {
                    if let Some(text) = message.pointer("/params/delta").and_then(Value::as_str) {
                        delta.push_str(text);
                    } else if let Some(text) =
                        message.pointer("/params/text").and_then(Value::as_str)
                    {
                        delta.push_str(text);
                    }
                }
                "item/completed" => {
                    let Some(item_type) =
                        message.pointer("/params/item/type").and_then(Value::as_str)
                    else {
                        continue;
                    };
                    if item_type == "agentMessage" {
                        if let Some(text) =
                            message.pointer("/params/item/text").and_then(Value::as_str)
                        {
                            completed_text = Some(text.to_string());
                        }
                    }
                }
                "turn/completed" => {
                    let turn_matches = message
                        .pointer("/params/turn/id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id == turn_id);
                    if turn_matches {
                        let text = completed_text.unwrap_or(delta);
                        let trimmed = text.trim();
                        if trimmed.is_empty() {
                            return Err(CapTraceError::Llm(
                                "codex app-server turn completed without agent message".to_string(),
                            ));
                        }
                        return Ok(trimmed.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), CapTraceError> {
        self.send_json(JsonRpcNotification { method, params })
        .await
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        max_wait: Duration,
    ) -> Result<Value, CapTraceError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_json(JsonRpcRequest { method, id, params })
        .await?;

        let mut deferred = VecDeque::new();
        loop {
            let message = if let Some(msg) = self.pending.pop_front() {
                msg
            } else {
                self.read_wire_message(max_wait).await?
            };

            if message
                .get("id")
                .and_then(Value::as_u64)
                .is_some_and(|msg_id| msg_id == id)
            {
                while let Some(msg) = deferred.pop_front() {
                    self.pending.push_back(msg);
                }

                if let Some(error) = message.get("error") {
                    return Err(CapTraceError::Llm(format!(
                        "codex app-server `{method}` failed: {error}"
                    )));
                }
                return message.get("result").cloned().ok_or_else(|| {
                    CapTraceError::Llm(format!(
                        "codex app-server `{method}` missing `result` in response"
                    ))
                });
            }

            deferred.push_back(message);
        }
    }

    async fn next_message(&mut self, max_wait: Duration) -> Result<Value, CapTraceError> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(message);
        }
        self.read_wire_message(max_wait).await
    }

    async fn read_wire_message(&mut self, max_wait: Duration) -> Result<Value, CapTraceError> {
        loop {
            let frame = timeout(max_wait, self.socket.next())
                .await
                .map_err(|_| CapTraceError::Llm("codex app-server read timeout".to_string()))?
                .ok_or_else(|| {
                    CapTraceError::Llm("codex app-server connection closed".to_string())
                })?
                .map_err(|err| {
                    CapTraceError::Llm(format!("codex app-server read failed: {err}"))
                })?;

            match frame {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    return serde_json::from_str(text.as_ref())
                        .map_err(|err| CapTraceError::Llm(format!("invalid JSON frame: {err}")));
                }
                tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                    return serde_json::from_slice(&bytes)
                        .map_err(|err| CapTraceError::Llm(format!("invalid JSON frame: {err}")));
                }
                tokio_tungstenite::tungstenite::Message::Ping(payload) => {
                    self.socket
                        .send(tokio_tungstenite::tungstenite::Message::Pong(payload))
                        .await
                        .map_err(|err| {
                            CapTraceError::Llm(format!("codex app-server pong failed: {err}"))
                        })?;
                }
                tokio_tungstenite::tungstenite::Message::Pong(_) => {}
                tokio_tungstenite::tungstenite::Message::Close(_) => {
                    return Err(CapTraceError::Llm(
                        "codex app-server websocket closed".to_string(),
                    ))
                }
                tokio_tungstenite::tungstenite::Message::Frame(_) => {}
            }
        }
    }

    async fn send_json<T: Serialize>(&mut self, value: T) -> Result<(), CapTraceError> {
        let text = serde_json::to_string(&value)
            .map_err(|err| CapTraceError::Llm(format!("serialize rpc message failed: {err}")))?;
        self.socket
            .send(tokio_tungstenite::tungstenite::Message::Text(text))
            .await
            .map_err(|err| CapTraceError::Llm(format!("codex app-server write failed: {err}")))
    }
}

fn to_json_value<T: Serialize>(value: T, context: &str) -> Value {
    serde_json::to_value(value)
        .unwrap_or_else(|err| panic!("failed to serialize {context}: {err}"))
}

fn parse_cases_from_agent_message(raw: &str) -> Result<Vec<String>, CapTraceError> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return cases_from_json(value);
    }

    let Some(start) = raw.find('{') else {
        return Err(CapTraceError::Llm(
            "codex app-server returned non-JSON output".to_string(),
        ));
    };
    let Some(end) = raw.rfind('}') else {
        return Err(CapTraceError::Llm(
            "codex app-server returned malformed JSON output".to_string(),
        ));
    };

    let sliced = &raw[start..=end];
    let value = serde_json::from_str::<Value>(sliced).map_err(|err| {
        CapTraceError::Llm(format!("failed to parse app-server JSON output: {err}"))
    })?;
    cases_from_json(value)
}

fn cases_from_json(value: Value) -> Result<Vec<String>, CapTraceError> {
    let Some(cases) = value.get("cases").and_then(Value::as_array) else {
        return Err(CapTraceError::Llm(
            "codex app-server output missing `cases` array".to_string(),
        ));
    };

    let out: Vec<String> = cases
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect();
    if out.is_empty() {
        return Err(CapTraceError::Llm(
            "codex app-server returned empty `cases`".to_string(),
        ));
    }
    Ok(out)
}
