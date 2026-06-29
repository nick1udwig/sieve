use super::client::{AppServerClient, AppServerClientConfig};
use super::{
    session_name_from_instruction, summarize_instruction, CodexSessionStore, CodexTurnRecord,
    StoredCodexSession,
};
mod active;
mod config;
mod events;
mod protocol;
mod stream;

use crate::json_util::to_json_value;
use crate::logging::now_ms;
use active::ActiveCodexTurn;
use async_trait::async_trait;
use config::CodexManagerConfig;
use protocol::*;
use sieve_runtime::{CodexExecToolResult, CodexSessionToolResult, CodexTool, RuntimeEventLog};
use sieve_types::{
    CodexExecRequest, CodexSandboxMode, CodexSessionLifecycleStatus, CodexSessionRequest,
    CodexTurnResult, PlannerCodexSession, RunId,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::{sync::mpsc, time::Duration};
use uuid::Uuid;

use sieve_runtime::ApprovalBus;

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

impl CodexManager {
    pub(crate) fn new(
        store_path: impl Into<PathBuf>,
        approval_bus: Arc<dyn ApprovalBus>,
        event_log: Arc<dyn RuntimeEventLog>,
    ) -> Result<Self, String> {
        Ok(Self {
            config: CodexManagerConfig::from_env(),
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

        let streamed_result = tokio::time::timeout(
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
