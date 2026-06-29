use super::client::{AppServerClient, AppServerClientConfig};
use super::{
    session_name_from_instruction, summarize_instruction, CodexSessionStore, CodexTurnRecord,
    StoredCodexSession,
};
mod protocol;

use crate::json_util::to_json_value;
use crate::logging::now_ms;
use async_trait::async_trait;
use protocol::*;
use sieve_runtime::{CodexExecToolResult, CodexSessionToolResult, CodexTool, RuntimeEventLog};
use sieve_types::{
    ApprovalAction, ApprovalPromptKind, ApprovalRequestId, ApprovalRequestedEvent,
    ApprovalResolvedEvent, CodexExecRequest, CodexSandboxMode, CodexSessionLifecycleStatus,
    CodexSessionRequest, CodexSessionStatusEvent, CodexTurnResult, CommandSegment,
    PlannerCodexSession, RunId, RuntimeEvent,
};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use sieve_runtime::ApprovalBus;
use std::future::Future;

const DEFAULT_CODEX_PROGRAM: &str = "codex";
const DEFAULT_CODEX_CONNECT_TIMEOUT_MS: u64 = 5 * 1000;
const DEFAULT_TURN_TIMEOUT_MS: u64 = 15 * 60 * 1000;
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 30 * 1000;

#[derive(Debug, Clone)]
struct CodexManagerConfig {
    program: Option<String>,
    ws_url: Option<String>,
    connect_timeout_ms: u64,
    model: Option<String>,
    turn_timeout_ms: u64,
    request_timeout_ms: u64,
}

pub(crate) struct CodexManager {
    config: CodexManagerConfig,
    store: CodexSessionStore,
    approval_bus: Arc<dyn ApprovalBus>,
    event_log: Arc<dyn RuntimeEventLog>,
    active_sessions: Mutex<HashMap<String, ActiveCodexTurn>>,
}

#[derive(Debug, Clone)]
struct SessionRunContext {
    run_id: RunId,
    session_id: Option<String>,
    session_name: String,
    thread_id: Option<String>,
    instruction: String,
    sandbox: CodexSandboxMode,
    cwd: Option<String>,
    writable_roots: Vec<String>,
    local_images: Vec<String>,
    persist_session: bool,
    task_summary: String,
    created_at_ms: u64,
}

impl SessionRunContext {
    fn stored_cwd(&self) -> String {
        self.cwd.clone().unwrap_or_else(|| ".".to_string())
    }
}

#[derive(Clone)]
struct ActiveCodexTurn {
    session_name: String,
    steer_tx: mpsc::UnboundedSender<SteerCommand>,
}

struct SteerCommand {
    text: String,
    result_tx: oneshot::Sender<Result<(), String>>,
}

impl CodexManager {
    pub(crate) fn new(
        store_path: impl Into<PathBuf>,
        approval_bus: Arc<dyn ApprovalBus>,
        event_log: Arc<dyn RuntimeEventLog>,
    ) -> Result<Self, String> {
        Ok(Self {
            config: CodexManagerConfig {
                program: env::var("SIEVE_CODEX_APP_SERVER_BIN")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .or_else(|| Some(DEFAULT_CODEX_PROGRAM.to_string())),
                ws_url: env::var("SIEVE_CODEX_APP_SERVER_WS_URL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                connect_timeout_ms: env::var("SIEVE_CODEX_APP_SERVER_CONNECT_TIMEOUT_MS")
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_CODEX_CONNECT_TIMEOUT_MS),
                model: env::var("SIEVE_CODEX_MODEL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                turn_timeout_ms: env::var("SIEVE_CODEX_TURN_TIMEOUT_MS")
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_TURN_TIMEOUT_MS),
                request_timeout_ms: env::var("SIEVE_CODEX_REQUEST_TIMEOUT_MS")
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_REQUEST_TIMEOUT_MS),
            },
            store: CodexSessionStore::new(store_path.into())?,
            approval_bus,
            event_log,
            active_sessions: Mutex::new(HashMap::new()),
        })
    }

    async fn exec_internal(
        &self,
        request: CodexExecRequest,
    ) -> Result<CodexExecToolResult, String> {
        let request = normalize_exec_request(request)?;
        let mut client = AppServerClient::spawn(&AppServerClientConfig {
            program: self.config.program.clone(),
            ws_url: self.config.ws_url.clone(),
            connect_timeout: Duration::from_millis(self.config.connect_timeout_ms),
        })
        .await?;
        self.call_with_timeout("initialize", client.initialize())
            .await?;
        let timeout_ms = request
            .timeout_ms
            .map(i64::try_from)
            .transpose()
            .map_err(|_| "codex exec timeout_ms exceeds i64 range".to_string())?;
        let response = self
            .call_with_timeout(
                "command/exec",
                client.request(
                    "command/exec",
                    to_json_value(
                        CommandExecParams {
                            command: &request.command,
                            cwd: request.cwd.as_deref(),
                            timeout_ms,
                            sandbox_policy: sandbox_policy_json(
                                request.sandbox,
                                request.cwd.as_deref(),
                                &request.writable_roots,
                            ),
                        },
                        "codex command exec params",
                    ),
                ),
            )
            .await?;
        Ok(CodexExecToolResult {
            result: decode_command_exec_result(&response)?,
        })
    }

    async fn task_internal(
        &self,
        request: CodexSessionRequest,
    ) -> Result<CodexSessionToolResult, String> {
        let request = normalize_session_request(request)?;
        let existing_names = self.store.existing_session_names()?;
        let session_name = session_name_from_instruction(&request.instruction, &existing_names);
        let run_ctx = SessionRunContext {
            run_id: RunId(format!("codex-task-{}", Uuid::new_v4())),
            session_id: None,
            session_name,
            thread_id: None,
            instruction: request.instruction,
            sandbox: request.sandbox,
            cwd: request.cwd,
            writable_roots: request.writable_roots,
            local_images: request.local_images,
            persist_session: false,
            task_summary: String::new(),
            created_at_ms: now_ms(),
        };
        self.run_codex_turn(run_ctx).await
    }

    async fn session_internal(
        &self,
        request: CodexSessionRequest,
    ) -> Result<CodexSessionToolResult, String> {
        let request = normalize_session_request(request)?;
        let resume_session_id = request
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let now = now_ms();
        let (session_id, thread_id, session_name, task_summary, created_at_ms, default_cwd) =
            if let Some(session_id) = resume_session_id {
                let stored = self
                    .store
                    .session(&session_id)?
                    .ok_or_else(|| format!("unknown codex session `{session_id}`"))?;
                (
                    session_id,
                    Some(stored.thread_id),
                    stored.session_name,
                    stored.task_summary,
                    stored.created_at_ms,
                    usable_stored_cwd(&stored.cwd),
                )
            } else {
                let existing_names = self.store.existing_session_names()?;
                (
                    format!("codex-session-{}", Uuid::new_v4()),
                    None,
                    session_name_from_instruction(&request.instruction, &existing_names),
                    summarize_instruction(&request.instruction),
                    now,
                    None,
                )
            };
        let run_ctx = SessionRunContext {
            run_id: RunId(format!("codex-session-{}", Uuid::new_v4())),
            session_id: Some(session_id),
            session_name,
            thread_id,
            instruction: request.instruction,
            sandbox: request.sandbox,
            cwd: request.cwd.or(default_cwd),
            writable_roots: request.writable_roots,
            local_images: request.local_images,
            persist_session: true,
            task_summary,
            created_at_ms,
        };
        let tool_result = match self.run_codex_turn(run_ctx.clone()).await {
            Ok(result) => result,
            Err(err) => {
                if let Some(session_id) = &run_ctx.session_id {
                    if let Some(thread_id) = run_ctx.thread_id.clone() {
                        let _ = self.store.upsert_session(&StoredCodexSession {
                            session_id: session_id.clone(),
                            thread_id,
                            session_name: run_ctx.session_name.clone(),
                            cwd: run_ctx.stored_cwd(),
                            sandbox: run_ctx.sandbox,
                            task_summary: run_ctx.task_summary.clone(),
                            last_result_summary: Some(err.clone()),
                            status: "failed".to_string(),
                            created_at_ms: run_ctx.created_at_ms,
                            updated_at_ms: now_ms(),
                        });
                    }
                }
                self.emit_status_event(
                    &run_ctx,
                    CodexSessionLifecycleStatus::Failed,
                    err.clone(),
                    Some(err.clone()),
                )
                .await?;
                return Err(err);
            }
        };
        if let Some(session_id) = &run_ctx.session_id {
            self.persist_session_result(session_id, &tool_result.result, &run_ctx, now)?;
        }
        Ok(tool_result)
    }

    fn persist_session_result(
        &self,
        session_id: &str,
        result: &CodexTurnResult,
        run_ctx: &SessionRunContext,
        updated_at_ms: u64,
    ) -> Result<(), String> {
        let Some(thread_id) = result
            .thread_id
            .clone()
            .or_else(|| run_ctx.thread_id.clone())
        else {
            return Err("missing codex thread id for persistent session".to_string());
        };
        self.store.upsert_session(&StoredCodexSession {
            session_id: session_id.to_string(),
            thread_id,
            session_name: result.session_name.clone(),
            cwd: run_ctx.stored_cwd(),
            sandbox: run_ctx.sandbox,
            task_summary: run_ctx.task_summary.clone(),
            last_result_summary: Some(result.summary.clone()),
            status: result.status.as_str().to_string(),
            created_at_ms: run_ctx.created_at_ms,
            updated_at_ms,
        })?;
        Ok(())
    }

    async fn run_codex_turn(
        &self,
        run_ctx: SessionRunContext,
    ) -> Result<CodexSessionToolResult, String> {
        let mut client = AppServerClient::spawn(&AppServerClientConfig {
            program: self.config.program.clone(),
            ws_url: self.config.ws_url.clone(),
            connect_timeout: Duration::from_millis(self.config.connect_timeout_ms),
        })
        .await?;
        self.call_with_timeout("initialize", client.initialize())
            .await?;

        let thread_id = self.start_or_resume_thread(&mut client, &run_ctx).await?;

        if run_ctx.persist_session {
            let session_id = run_ctx
                .session_id
                .clone()
                .ok_or_else(|| "missing codex session id".to_string())?;
            self.store.upsert_session(&StoredCodexSession {
                session_id,
                thread_id: thread_id.clone(),
                session_name: run_ctx.session_name.clone(),
                cwd: run_ctx.stored_cwd(),
                sandbox: run_ctx.sandbox,
                task_summary: run_ctx.task_summary.clone(),
                last_result_summary: None,
                status: "running".to_string(),
                created_at_ms: run_ctx.created_at_ms,
                updated_at_ms: now_ms(),
            })?;
            self.emit_status_event(
                &run_ctx,
                CodexSessionLifecycleStatus::Running,
                format!(
                    "starting bounded increment in {}",
                    run_ctx.cwd.as_deref().unwrap_or(".")
                ),
                None,
            )
            .await?;
        }

        let turn_response = self
            .call_with_timeout(
                "turn/start",
                client.request(
                    "turn/start",
                    to_json_value(
                        TurnStartParams {
                            thread_id: &thread_id,
                            input: build_turn_input(&run_ctx.instruction, &run_ctx.local_images),
                            cwd: run_ctx.cwd.as_deref(),
                            approval_policy: "on-request",
                            sandbox_policy: sandbox_policy_json(
                                run_ctx.sandbox,
                                run_ctx.cwd.as_deref(),
                                &run_ctx.writable_roots,
                            ),
                            model: &self.config.model,
                            output_schema: structured_turn_output_schema(),
                        },
                        "codex turn start params",
                    ),
                ),
            )
            .await?;
        let turn_id = turn_response
            .pointer("/turn/id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "codex turn/start missing turn.id".to_string())?
            .to_string();
        let (steer_tx, mut steer_rx) = mpsc::unbounded_channel();
        if let Some(session_id) = run_ctx.session_id.as_deref() {
            self.register_active_session(session_id, &run_ctx.session_name, steer_tx)?;
        }

        let streamed_result = timeout(
            Duration::from_millis(self.config.turn_timeout_ms),
            self.wait_for_turn_completion(
                &mut client,
                &run_ctx,
                &thread_id,
                &turn_id,
                &mut steer_rx,
            ),
        )
        .await;
        self.unregister_active_session(run_ctx.session_id.as_deref());
        let streamed = streamed_result.map_err(|_| {
            format!(
                "codex turn timed out after {}ms",
                self.config.turn_timeout_ms
            )
        })??;

        let parsed = decode_turn_result(
            run_ctx.session_id.clone(),
            run_ctx.session_name.clone(),
            Some(turn_id.clone()),
            Some(thread_id.clone()),
            streamed.final_text.trim(),
            streamed.turn_status.as_deref(),
            streamed.turn_error.as_deref(),
        );

        if run_ctx.persist_session {
            let session_id = run_ctx
                .session_id
                .clone()
                .ok_or_else(|| "missing codex session id".to_string())?;
            self.store.record_turn(CodexTurnRecord {
                session_id: &session_id,
                turn_id: parsed.turn_id.as_deref(),
                instruction: &run_ctx.instruction,
                status: parsed.status.as_str(),
                summary: &parsed.summary,
                user_visible: parsed.user_visible.as_deref(),
                created_at_ms: run_ctx.created_at_ms,
                completed_at_ms: now_ms(),
            })?;
            self.emit_status_event(
                &run_ctx,
                lifecycle_status_from_turn_status(parsed.status.clone()),
                parsed
                    .user_visible
                    .clone()
                    .unwrap_or_else(|| parsed.summary.clone()),
                Some(parsed.summary.clone()),
            )
            .await?;
        }

        Ok(CodexSessionToolResult { result: parsed })
    }

    async fn start_or_resume_thread(
        &self,
        client: &mut AppServerClient,
        run_ctx: &SessionRunContext,
    ) -> Result<String, String> {
        if let Some(thread_id) = &run_ctx.thread_id {
            match self
                .call_with_timeout(
                    "thread/resume",
                    client.request(
                        "thread/resume",
                        to_json_value(
                            ThreadResumeParams { thread_id },
                            "codex thread resume params",
                        ),
                    ),
                )
                .await
            {
                Ok(_) => return Ok(thread_id.clone()),
                Err(err) if is_missing_rollout_error(&err) => {}
                Err(err) => return Err(err),
            }
        }
        self.start_thread(client, run_ctx).await
    }

    async fn start_thread(
        &self,
        client: &mut AppServerClient,
        run_ctx: &SessionRunContext,
    ) -> Result<String, String> {
        let response = self
            .call_with_timeout(
                "thread/start",
                client.request(
                    "thread/start",
                    to_json_value(
                        ThreadStartParams {
                            cwd: run_ctx.cwd.as_deref(),
                            model: &self.config.model,
                            approval_policy: "on-request",
                            ephemeral: !run_ctx.persist_session,
                        },
                        "codex thread start params",
                    ),
                ),
            )
            .await?;
        let thread_id = response
            .pointer("/thread/id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "codex thread/start missing thread.id".to_string())?
            .to_string();
        self.call_with_timeout(
            "thread/name/set",
            client.request(
                "thread/name/set",
                to_json_value(
                    ThreadNameSetParams {
                        thread_id: &thread_id,
                        name: &run_ctx.session_name,
                    },
                    "codex thread name set params",
                ),
            ),
        )
        .await?;
        Ok(thread_id)
    }

    fn register_active_session(
        &self,
        session_id: &str,
        session_name: &str,
        steer_tx: mpsc::UnboundedSender<SteerCommand>,
    ) -> Result<(), String> {
        self.active_sessions
            .lock()
            .map_err(|_| "codex active session lock poisoned".to_string())?
            .insert(
                session_id.to_string(),
                ActiveCodexTurn {
                    session_name: session_name.to_string(),
                    steer_tx,
                },
            );
        Ok(())
    }

    fn unregister_active_session(&self, session_id: Option<&str>) {
        let Some(session_id) = session_id else {
            return;
        };
        if let Ok(mut active) = self.active_sessions.lock() {
            active.remove(session_id);
        }
    }

    async fn steer_session_internal(&self, session_id: &str, text: &str) -> Result<(), String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("codex steer text must be non-empty".to_string());
        }
        let active = self
            .active_sessions
            .lock()
            .map_err(|_| "codex active session lock poisoned".to_string())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("codex session `{session_id}` is not currently running"))?;
        let (result_tx, result_rx) = oneshot::channel();
        active
            .steer_tx
            .send(SteerCommand {
                text: trimmed.to_string(),
                result_tx,
            })
            .map_err(|_| {
                format!(
                    "codex session `{}` is no longer accepting steer input",
                    active.session_name
                )
            })?;
        result_rx.await.map_err(|_| {
            format!(
                "codex session `{}` dropped steer response channel",
                active.session_name
            )
        })?
    }

    async fn wait_for_turn_completion(
        &self,
        client: &mut AppServerClient,
        run_ctx: &SessionRunContext,
        thread_id: &str,
        turn_id: &str,
        steer_rx: &mut mpsc::UnboundedReceiver<SteerCommand>,
    ) -> Result<TurnCompletion, String> {
        let mut state = TurnStreamState::default();
        loop {
            tokio::select! {
                maybe_steer = steer_rx.recv() => {
                    let Some(steer) = maybe_steer else {
                        continue;
                    };
                    let result = self
                        .request_turn_steer(client, run_ctx, thread_id, turn_id, &steer.text)
                        .await;
                    let _ = steer.result_tx.send(result);
                }
                message = client.next_message() => {
                    let message = message?;
                    if let Some(method) = message.get("method").and_then(serde_json::Value::as_str) {
                        self.record_session_event(run_ctx, turn_id, method, &message)
                            .await?;
                        if message.get("id").is_some() {
                            self.handle_server_request(client, run_ctx, &message, &mut state)
                                .await?;
                            continue;
                        }
                        match method {
                            "item/agentMessage/delta" => {
                                if let Some(text) = message
                                    .pointer("/params/delta")
                                    .and_then(serde_json::Value::as_str)
                                    .or_else(|| {
                                        message
                                            .pointer("/params/text")
                                            .and_then(serde_json::Value::as_str)
                                    })
                                {
                                    state.agent_delta.push_str(text);
                                }
                            }
                            "item/started" | "item/completed" => {
                                handle_item_notification(&message, &mut state);
                            }
                            "turn/completed" => {
                                let completed_thread_id = message
                                    .pointer("/params/turn/threadId")
                                    .and_then(serde_json::Value::as_str)
                                    .or_else(|| {
                                        message
                                            .pointer("/params/turn/thread_id")
                                            .and_then(serde_json::Value::as_str)
                                    });
                                let completed_turn_id = message
                                    .pointer("/params/turn/id")
                                    .and_then(serde_json::Value::as_str);
                                if completed_turn_id == Some(turn_id)
                                    || (completed_thread_id == Some(thread_id)
                                        && completed_turn_id.is_none())
                                {
                                    let final_text = state
                                        .final_agent_text
                                        .clone()
                                        .unwrap_or_else(|| state.agent_delta.trim().to_string());
                                    return Ok(TurnCompletion {
                                        final_text,
                                        turn_status: message
                                            .pointer("/params/turn/status")
                                            .and_then(serde_json::Value::as_str)
                                            .map(ToString::to_string),
                                        turn_error: message
                                            .pointer("/params/turn/error/message")
                                            .and_then(serde_json::Value::as_str)
                                            .map(ToString::to_string),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    async fn request_turn_steer(
        &self,
        client: &mut AppServerClient,
        run_ctx: &SessionRunContext,
        thread_id: &str,
        turn_id: &str,
        text: &str,
    ) -> Result<(), String> {
        self.call_with_timeout(
            "turn/steer",
            client.request(
                "turn/steer",
                to_json_value(
                    TurnSteerParams {
                        thread_id,
                        input: vec![TurnInputItem {
                            item_type: "text",
                            text: Some(text.to_string()),
                            path: None,
                        }],
                        expected_turn_id: turn_id,
                    },
                    "codex turn steer params",
                ),
            ),
        )
        .await?;
        self.emit_status_event(
            run_ctx,
            CodexSessionLifecycleStatus::Running,
            format!("received user steer: {}", truncate_preview(text, 160)),
            None,
        )
        .await?;
        Ok(())
    }

    async fn handle_server_request(
        &self,
        client: &mut AppServerClient,
        run_ctx: &SessionRunContext,
        message: &serde_json::Value,
        state: &mut TurnStreamState,
    ) -> Result<(), String> {
        let id = message
            .get("id")
            .cloned()
            .ok_or_else(|| "codex server request missing id".to_string())?;
        let method = message
            .get("method")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "codex server request missing method".to_string())?;
        match method {
            "item/commandExecution/requestApproval" => {
                let command = message
                    .pointer("/params/command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("command");
                let reason = message
                    .pointer("/params/reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Codex requested approval to run a command")
                    .to_string();
                let decision = self
                    .request_user_approval(
                        &run_ctx.run_id,
                        run_ctx,
                        ApprovalPromptKind::Command,
                        vec![CommandSegment {
                            argv: vec![command.to_string()],
                            operator_before: None,
                        }],
                        reason,
                        None,
                    )
                    .await?;
                client
                    .respond(
                        id,
                        to_json_value(
                            ApprovalDecision {
                                decision: approval_action_to_codex_decision(decision),
                            },
                            "codex command approval decision",
                        ),
                    )
                    .await?;
            }
            "item/fileChange/requestApproval" => {
                let item_id = message
                    .pointer("/params/itemId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let reason = message
                    .pointer("/params/reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Codex requested approval to apply file changes")
                    .to_string();
                let preview = state.file_change_previews.get(item_id).cloned();
                let decision = self
                    .request_user_approval(
                        &run_ctx.run_id,
                        run_ctx,
                        ApprovalPromptKind::FileChange,
                        Vec::new(),
                        reason,
                        preview,
                    )
                    .await?;
                client
                    .respond(
                        id,
                        to_json_value(
                            ApprovalDecision {
                                decision: approval_action_to_codex_decision(decision),
                            },
                            "codex file change approval decision",
                        ),
                    )
                    .await?;
            }
            other => {
                return Err(format!(
                    "[{}] unsupported codex server request `{other}`",
                    run_ctx.session_name
                ));
            }
        }
        Ok(())
    }

    async fn request_user_approval(
        &self,
        run_id: &RunId,
        run_ctx: &SessionRunContext,
        prompt_kind: ApprovalPromptKind,
        command_segments: Vec<CommandSegment>,
        reason: String,
        preview: Option<String>,
    ) -> Result<ApprovalAction, String> {
        let request_id = ApprovalRequestId(format!(
            "{}-approval-{}",
            run_ctx.session_name,
            Uuid::new_v4()
        ));
        let requested = ApprovalRequestedEvent {
            schema_version: 1,
            request_id: request_id.clone(),
            run_id: run_id.clone(),
            prompt_kind,
            title: Some(run_ctx.session_name.clone()),
            command_segments,
            inferred_capabilities: Vec::new(),
            blocked_rule_id: match prompt_kind {
                ApprovalPromptKind::Command => "codex_command_approval".to_string(),
                ApprovalPromptKind::FileChange => "codex_file_change_approval".to_string(),
            },
            reason,
            preview,
            reply_to_session_id: run_ctx.session_id.clone(),
            allow_approve_always: true,
            created_at_ms: now_ms(),
        };
        self.append_runtime_event(RuntimeEvent::ApprovalRequested(requested.clone()))
            .await?;
        self.emit_status_event(
            run_ctx,
            CodexSessionLifecycleStatus::WaitingApproval,
            requested.reason.clone(),
            None,
        )
        .await?;
        self.approval_bus
            .publish_requested(requested)
            .await
            .map_err(|err| format!("publish codex approval request failed: {err}"))?;
        let resolved = self
            .approval_bus
            .wait_resolved(&request_id)
            .await
            .map_err(|err| format!("wait codex approval resolution failed: {err}"))?;
        self.append_runtime_event(RuntimeEvent::ApprovalResolved(ApprovalResolvedEvent {
            schema_version: 1,
            request_id: resolved.request_id.clone(),
            run_id: run_id.clone(),
            action: resolved.action,
            created_at_ms: now_ms(),
        }))
        .await?;
        self.emit_status_event(
            run_ctx,
            match resolved.action {
                ApprovalAction::Deny => CodexSessionLifecycleStatus::Failed,
                ApprovalAction::ApproveOnce | ApprovalAction::ApproveAlways => {
                    CodexSessionLifecycleStatus::Running
                }
            },
            match resolved.action {
                ApprovalAction::Deny => "approval denied".to_string(),
                ApprovalAction::ApproveOnce => "approval granted once".to_string(),
                ApprovalAction::ApproveAlways => "approval granted always".to_string(),
            },
            None,
        )
        .await?;
        Ok(resolved.action)
    }

    async fn emit_status_event(
        &self,
        run_ctx: &SessionRunContext,
        status: CodexSessionLifecycleStatus,
        last_step: String,
        summary: Option<String>,
    ) -> Result<(), String> {
        let Some(session_id) = run_ctx.session_id.clone() else {
            return Ok(());
        };
        let updated_at_ms = now_ms();
        if run_ctx.persist_session {
            if let Some(mut stored) = self.store.session(&session_id)? {
                stored.status = match status {
                    CodexSessionLifecycleStatus::Running => "running".to_string(),
                    CodexSessionLifecycleStatus::WaitingApproval => "waiting_approval".to_string(),
                    CodexSessionLifecycleStatus::NeedsFollowup => "needs_followup".to_string(),
                    CodexSessionLifecycleStatus::Completed => "completed".to_string(),
                    CodexSessionLifecycleStatus::Failed => "failed".to_string(),
                };
                stored.updated_at_ms = updated_at_ms;
                if let Some(summary) = summary.clone().or_else(|| {
                    matches!(
                        status,
                        CodexSessionLifecycleStatus::WaitingApproval
                            | CodexSessionLifecycleStatus::Failed
                    )
                    .then(|| last_step.clone())
                }) {
                    stored.last_result_summary = Some(summary);
                }
                self.store.upsert_session(&stored)?;
            }
        }
        self.append_runtime_event(RuntimeEvent::CodexSessionStatus(CodexSessionStatusEvent {
            schema_version: 1,
            run_id: run_ctx.run_id.clone(),
            session_id,
            session_name: run_ctx.session_name.clone(),
            cwd: run_ctx.cwd.clone(),
            status,
            started_at_ms: run_ctx.created_at_ms,
            updated_at_ms,
            last_step,
            summary,
        }))
        .await
    }

    async fn append_runtime_event(&self, event: RuntimeEvent) -> Result<(), String> {
        self.event_log
            .append(event)
            .await
            .map_err(format_event_log_error)
    }

    async fn call_with_timeout<T>(
        &self,
        label: &str,
        future: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        timeout(
            Duration::from_millis(self.config.request_timeout_ms),
            future,
        )
        .await
        .map_err(|_| {
            format!(
                "{label} timed out after {}ms",
                self.config.request_timeout_ms
            )
        })?
    }

    async fn record_session_event(
        &self,
        run_ctx: &SessionRunContext,
        turn_id: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Result<(), String> {
        if !run_ctx.persist_session {
            return Ok(());
        }
        let Some(session_id) = run_ctx.session_id.as_deref() else {
            return Ok(());
        };
        self.store.record_event(
            session_id,
            Some(turn_id),
            event_type,
            &payload.to_string(),
            now_ms(),
        )
    }
}

#[async_trait]
impl CodexTool for CodexManager {
    async fn exec(&self, request: CodexExecRequest) -> Result<CodexExecToolResult, String> {
        self.exec_internal(request).await
    }

    async fn run_task(
        &self,
        request: CodexSessionRequest,
    ) -> Result<CodexSessionToolResult, String> {
        self.task_internal(request).await
    }

    async fn run_session(
        &self,
        request: CodexSessionRequest,
    ) -> Result<CodexSessionToolResult, String> {
        self.session_internal(request).await
    }

    async fn steer_session(&self, session_id: &str, text: &str) -> Result<(), String> {
        self.steer_session_internal(session_id, text).await
    }

    async fn planner_sessions(&self) -> Result<Vec<PlannerCodexSession>, String> {
        self.store.planner_sessions(12)
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
