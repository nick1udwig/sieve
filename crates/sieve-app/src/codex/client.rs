use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

#[derive(Debug, Clone)]
pub(crate) struct AppServerClientConfig {
    pub(crate) program: Option<String>,
    pub(crate) ws_url: Option<String>,
    pub(crate) connect_timeout: Duration,
}

pub(crate) struct AppServerClient {
    transport: AppServerTransport,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    pending: VecDeque<Value>,
    next_id: u64,
}

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

enum AppServerTransport {
    Stdio {
        #[allow(dead_code)]
        child: Child,
        stdin: ChildStdin,
        stdout: Lines<BufReader<ChildStdout>>,
    },
    WebSocket {
        socket: WsStream,
    },
}

#[derive(Serialize)]
struct InitializeParams<'a> {
    #[serde(rename = "clientInfo")]
    client_info: ClientInfo<'a>,
    capabilities: ClientCapabilities,
}

#[derive(Serialize)]
struct ClientInfo<'a> {
    name: &'a str,
    title: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct ClientCapabilities {
    #[serde(rename = "experimentalApi")]
    experimental_api: bool,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a, T: Serialize> {
    id: u64,
    method: &'a str,
    params: T,
}

#[derive(Serialize)]
struct JsonRpcNotification<'a, T: Serialize> {
    method: &'a str,
    params: T,
}

#[derive(Serialize)]
struct JsonRpcResponse<T: Serialize> {
    id: Value,
    result: T,
}

impl AppServerClient {
    pub(crate) async fn spawn(config: &AppServerClientConfig) -> Result<Self, String> {
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        if let Some(ws_url) = config
            .ws_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let (socket, _) = timeout(config.connect_timeout, connect_async(ws_url))
                .await
                .map_err(|_| format!("codex app-server connect timeout at {ws_url}"))?
                .map_err(|err| format!("codex app-server not reachable at {ws_url}: {err}"))?;
            return Ok(Self {
                transport: AppServerTransport::WebSocket { socket },
                stderr_lines,
                pending: VecDeque::new(),
                next_id: 1,
            });
        }

        let program = config
            .program
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "missing codex app-server program".to_string())?;
        let mut command = Command::new(program);
        if should_add_codex_app_server_args(program) {
            command.arg("app-server").arg("--listen").arg("stdio://");
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|err| format!("spawn codex app-server failed: {err}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "codex app-server stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "codex app-server stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "codex app-server stderr unavailable".to_string())?;
        let stderr_lines_task = stderr_lines.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut stored) = stderr_lines_task.lock() {
                    stored.push(line);
                    if stored.len() > 200 {
                        let drain = stored.len().saturating_sub(200);
                        stored.drain(0..drain);
                    }
                }
            }
        });

        Ok(Self {
            transport: AppServerTransport::Stdio {
                child,
                stdin,
                stdout: BufReader::new(stdout).lines(),
            },
            stderr_lines,
            pending: VecDeque::new(),
            next_id: 1,
        })
    }

    pub(crate) async fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            to_json_value(
                InitializeParams {
                    client_info: ClientInfo {
                        name: "sieve",
                        title: "Sieve",
                        version: env!("CARGO_PKG_VERSION"),
                    },
                    capabilities: ClientCapabilities {
                        experimental_api: true,
                    },
                },
                "codex initialize params",
            ),
        )
        .await?;
        self.notify("initialized", Value::Object(Default::default()))
            .await
    }

    pub(crate) async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.write_message(JsonRpcRequest { id, method, params })
            .await?;
        let mut deferred = VecDeque::new();
        loop {
            let message = if let Some(message) = self.pending.pop_front() {
                message
            } else {
                self.read_transport_message().await?
            };
            if matches_response_id(&message, id) {
                self.pending.extend(deferred);
                if let Some(result) = message.get("result") {
                    return Ok(result.clone());
                }
                if let Some(error) = message.get("error") {
                    return Err(format!(
                        "{method} failed: {}{}",
                        extract_error_message(error),
                        self.stderr_suffix()
                    ));
                }
                return Err(format!(
                    "{method} returned invalid response payload{}",
                    self.stderr_suffix()
                ));
            }
            deferred.push_back(message);
        }
    }

    pub(crate) async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write_message(JsonRpcNotification { method, params })
            .await
    }

    pub(crate) async fn respond(&mut self, id: Value, result: Value) -> Result<(), String> {
        self.write_message(JsonRpcResponse { id, result }).await
    }

    pub(crate) async fn next_message(&mut self) -> Result<Value, String> {
        if let Some(message) = self.pending.pop_front() {
            return Ok(message);
        }
        self.read_transport_message().await
    }

    async fn read_transport_message(&mut self) -> Result<Value, String> {
        match &mut self.transport {
            AppServerTransport::Stdio { stdout, .. } => loop {
                let Some(line) = stdout
                    .next_line()
                    .await
                    .map_err(|err| format!("read codex app-server output failed: {err}"))?
                else {
                    return Err(format!(
                        "codex app-server exited before completing request{}",
                        self.stderr_suffix()
                    ));
                };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: Value = serde_json::from_str(trimmed).map_err(|err| {
                    format!("decode codex app-server message failed: {err}; line={trimmed}")
                })?;
                return Ok(parsed);
            },
            AppServerTransport::WebSocket { socket } => loop {
                let Some(frame) = socket.next().await else {
                    return Err("codex app-server websocket connection closed".to_string());
                };
                let frame = frame.map_err(|err| format!("codex app-server read failed: {err}"))?;
                match frame {
                    tokio_tungstenite::tungstenite::Message::Text(text) => {
                        return serde_json::from_str(text.as_ref())
                            .map_err(|err| format!("invalid JSON frame: {err}"));
                    }
                    tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                        return serde_json::from_slice(&bytes)
                            .map_err(|err| format!("invalid JSON frame: {err}"));
                    }
                    tokio_tungstenite::tungstenite::Message::Ping(payload) => {
                        socket
                            .send(tokio_tungstenite::tungstenite::Message::Pong(payload))
                            .await
                            .map_err(|err| {
                                format!("codex app-server websocket pong failed: {err}")
                            })?;
                    }
                    tokio_tungstenite::tungstenite::Message::Pong(_) => {}
                    tokio_tungstenite::tungstenite::Message::Close(_) => {
                        return Err("codex app-server websocket closed".to_string());
                    }
                    tokio_tungstenite::tungstenite::Message::Frame(_) => {}
                }
            },
        }
    }

    fn stderr_suffix(&self) -> String {
        let stderr = self.stderr_snapshot();
        if stderr.is_empty() {
            String::new()
        } else {
            format!("; stderr: {stderr}")
        }
    }

    pub(crate) fn stderr_snapshot(&self) -> String {
        self.stderr_lines
            .lock()
            .ok()
            .map(|lines| lines.join(" | "))
            .unwrap_or_default()
    }

    async fn write_message<T: Serialize>(&mut self, value: T) -> Result<(), String> {
        let encoded = serde_json::to_string(&value)
            .map_err(|err| format!("encode codex app-server request failed: {err}"))?;
        match &mut self.transport {
            AppServerTransport::Stdio { stdin, .. } => {
                stdin
                    .write_all(encoded.as_bytes())
                    .await
                    .map_err(|err| format!("write codex app-server request failed: {err}"))?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|err| format!("write codex app-server newline failed: {err}"))?;
                stdin
                    .flush()
                    .await
                    .map_err(|err| format!("flush codex app-server request failed: {err}"))
            }
            AppServerTransport::WebSocket { socket } => socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    encoded.into(),
                ))
                .await
                .map_err(|err| format!("codex app-server write failed: {err}")),
        }
    }
}

fn to_json_value<T: Serialize>(value: T, context: &str) -> Value {
    serde_json::to_value(value).unwrap_or_else(|err| panic!("failed to serialize {context}: {err}"))
}

fn should_add_codex_app_server_args(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "codex")
}

fn matches_response_id(message: &Value, expected_id: u64) -> bool {
    message
        .get("id")
        .and_then(Value::as_u64)
        .is_some_and(|id| id == expected_id)
}

fn extract_error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown codex app-server error")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn websocket_config_can_skip_binary_spawn() {
        let config = AppServerClientConfig {
            program: None,
            ws_url: Some("ws://127.0.0.1:4500".to_string()),
            connect_timeout: Duration::from_millis(500),
        };
        assert_eq!(config.ws_url.as_deref(), Some("ws://127.0.0.1:4500"));
    }

    #[test]
    fn codex_binary_gets_app_server_args() {
        assert!(should_add_codex_app_server_args("codex"));
        assert!(should_add_codex_app_server_args("/usr/local/bin/codex"));
        assert!(!should_add_codex_app_server_args(
            "/tmp/mock-codex-server.py"
        ));
    }

    #[test]
    fn matches_response_id_reads_integer_ids() {
        assert!(matches_response_id(&json!({"id": 7, "result": {}}), 7));
        assert!(!matches_response_id(&json!({"id": 8, "result": {}}), 7));
    }
}
