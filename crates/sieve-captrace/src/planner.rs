#![forbid(unsafe_code)]

use crate::error::{llm_err, CapTraceError};
use crate::fixture::{
    TOKEN_ARG, TOKEN_DATA, TOKEN_HEADER, TOKEN_IN_FILE, TOKEN_IN_FILE_2, TOKEN_KV, TOKEN_OUT_FILE,
    TOKEN_TMP_DIR, TOKEN_URL,
};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sieve_llm::{OpenAiPlannerModel, PlannerModel};
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use sieve_tool_contracts::{validate_at_index, TypedCall};
use sieve_types::{CommandKnowledge, PlannerTurnInput, RunId};
use std::collections::{BTreeSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration, Instant};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

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

pub struct PlannerCaseGenerator {
    planner: Arc<dyn PlannerModel>,
    shell: BasicShellAnalyzer,
}

impl PlannerCaseGenerator {
    pub fn from_env() -> Result<Self, CapTraceError> {
        let planner = OpenAiPlannerModel::from_env().map_err(llm_err)?;
        Ok(Self {
            planner: Arc::new(planner),
            shell: BasicShellAnalyzer,
        })
    }

    #[cfg(test)]
    pub fn new(planner: Arc<dyn PlannerModel>) -> Self {
        Self {
            planner,
            shell: BasicShellAnalyzer,
        }
    }
}

#[async_trait]
impl CaseGenerator for PlannerCaseGenerator {
    async fn generate_cases(
        &self,
        request: CaseGenerationRequest,
    ) -> Result<Vec<Vec<String>>, CapTraceError> {
        let user_message = generation_prompt(&request.command, request.max_cases);
        let output = self
            .planner
            .plan_turn(PlannerTurnInput {
                run_id: RunId(format!("captrace-llm-{}", now_ms())),
                user_message,
                allowed_tools: vec!["bash".to_string()],
                allowed_net_connect_scopes: Vec::new(),
                previous_events: Vec::new(),
                guidance: None,
            })
            .await
            .map_err(llm_err)?;

        let mut unique = BTreeSet::new();
        let mut cases = Vec::new();
        for (idx, tool_call) in output.tool_calls.iter().enumerate() {
            let args_json = serde_json::to_value(&tool_call.args)
                .map_err(|err| CapTraceError::Llm(err.to_string()))?;
            let typed = validate_at_index(idx, &tool_call.tool_name, &args_json)
                .map_err(|err| CapTraceError::Llm(err.to_string()))?;
            let TypedCall::Bash(args) = typed else {
                continue;
            };
            let analysis = self
                .shell
                .analyze_shell_lc_script(&args.cmd)
                .map_err(|err| CapTraceError::Shell(err.to_string()))?;
            if analysis.knowledge != CommandKnowledge::Known || analysis.segments.len() != 1 {
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
                "planner returned no valid command cases".to_string(),
            ));
        }
        Ok(cases)
    }
}

pub struct CodexAppServerCaseGenerator {
    ws_url: String,
    model: String,
    connect_timeout: Duration,
    turn_timeout: Duration,
    shell: BasicShellAnalyzer,
}

impl CodexAppServerCaseGenerator {
    pub fn from_env() -> Self {
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

    pub async fn is_running(&self) -> bool {
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
            if analysis.knowledge != CommandKnowledge::Known || analysis.segments.len() != 1 {
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
            json!({
                "clientInfo": {
                    "name": "sieve_captrace",
                    "title": "Sieve CapTrace",
                    "version": "0.1.0"
                }
            }),
            Duration::from_secs(5),
        )
        .await?;
        self.notify("initialized", json!({})).await?;
        Ok(())
    }

    async fn start_thread(&mut self, model: &str) -> Result<String, CapTraceError> {
        let result = self
            .request(
                "thread/start",
                json!({
                    "model": model
                }),
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
                json!({
                    "threadId": thread_id,
                    "input": [
                        { "type": "text", "text": prompt }
                    ],
                    "outputSchema": {
                        "type": "object",
                        "required": ["cases"],
                        "additionalProperties": false,
                        "properties": {
                            "cases": {
                                "type": "array",
                                "items": { "type": "string" },
                                "minItems": 1
                            }
                        }
                    }
                }),
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
        self.send_json(&json!({
            "method": method,
            "params": params
        }))
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
        self.send_json(&json!({
            "method": method,
            "id": id,
            "params": params
        }))
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

    async fn send_json(&mut self, value: &Value) -> Result<(), CapTraceError> {
        let text = serde_json::to_string(value)
            .map_err(|err| CapTraceError::Llm(format!("serialize rpc message failed: {err}")))?;
        self.socket
            .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await
            .map_err(|err| CapTraceError::Llm(format!("codex app-server write failed: {err}")))
    }
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

fn generation_prompt(command: &str, max_cases: usize) -> String {
    format!(
        "Return JSON only with shape {{\"cases\": [string...]}}. Generate up to {max_cases} shell command strings. Each command must invoke `{command}` only. No pipes, no control operators, no shell variables. Use placeholders {TOKEN_TMP_DIR} {TOKEN_IN_FILE} {TOKEN_IN_FILE_2} {TOKEN_OUT_FILE} {TOKEN_URL} {TOKEN_HEADER} {TOKEN_DATA} {TOKEN_KV} {TOKEN_ARG}. Focus on valid command usage that should run successfully. Explore different subcommands and meaningful flag combinations. Avoid help/version and flags likely to be unsupported by the command unless no other runnable forms exist."
    )
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_u64_env(key: &str, default: u64) -> u64 {
    env_non_empty(key)
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(default)
}

pub(crate) fn argv_matches_command(argv: &[String], command: &str) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };

    if token_matches_command(first, command) {
        return true;
    }

    if first == "sudo" {
        if let Some(second) = argv.get(1) {
            return token_matches_command(second, command);
        }
    }

    false
}

fn token_matches_command(token: &str, command: &str) -> bool {
    if token == command || token.ends_with(&format!("/{command}")) {
        return true;
    }

    let Some(command_basename) = Path::new(command).file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    token == command_basename || token.ends_with(&format!("/{command_basename}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
