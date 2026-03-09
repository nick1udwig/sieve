use super::client::{AppServerClient, AppServerClientConfig};
use super::{
    session_name_from_instruction, summarize_instruction, CodexSessionStore, StoredCodexSession,
};
use async_trait::async_trait;
use sieve_runtime::{CodexTool, CodexToolResult, RuntimeEventLog};
use sieve_types::{
    ApprovalAction, ApprovalPromptKind, ApprovalRequestId, ApprovalRequestedEvent,
    ApprovalResolvedEvent, CodexExecRequest, CodexSandboxMode, CodexSessionRequest,
    CodexTurnResult, CodexTurnStatus, CommandSegment, PlannerCodexSession, RunId, RuntimeEvent,
};
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

use sieve_runtime::{ApprovalBus, EventLogError};

const DEFAULT_CODEX_PROGRAM: &str = "codex";
const DEFAULT_TURN_TIMEOUT_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Clone)]
struct CodexManagerConfig {
    program: String,
    model: Option<String>,
    turn_timeout_ms: u64,
}

pub(crate) struct CodexManager {
    config: CodexManagerConfig,
    store: CodexSessionStore,
    approval_bus: Arc<dyn ApprovalBus>,
    event_log: Arc<dyn RuntimeEventLog>,
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

#[derive(Debug, Default)]
struct TurnStreamState {
    agent_delta: String,
    final_agent_text: Option<String>,
    file_change_previews: HashMap<String, String>,
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
                    .unwrap_or_else(|| DEFAULT_CODEX_PROGRAM.to_string()),
                model: env::var("SIEVE_CODEX_MODEL")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                turn_timeout_ms: env::var("SIEVE_CODEX_TURN_TIMEOUT_MS")
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(DEFAULT_TURN_TIMEOUT_MS),
            },
            store: CodexSessionStore::new(store_path.into())?,
            approval_bus,
            event_log,
        })
    }

    async fn exec_internal(&self, request: CodexExecRequest) -> Result<CodexToolResult, String> {
        let existing_names = self.store.existing_session_names()?;
        let session_name = session_name_from_instruction(&request.instruction, &existing_names);
        let run_ctx = SessionRunContext {
            run_id: RunId(format!("codex-exec-{}", Uuid::new_v4())),
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
    ) -> Result<CodexToolResult, String> {
        let now = now_ms();
        let (
            session_id,
            thread_id,
            session_name,
            task_summary,
            created_at_ms,
            default_cwd,
            default_sandbox,
        ) = if let Some(session_id) = request.session_id.clone() {
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
                Some(stored.cwd),
                Some(stored.sandbox),
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
        let tool_result = self.run_codex_turn(run_ctx.clone()).await?;
        if let Some(session_id) = &run_ctx.session_id {
            self.persist_session_result(session_id, &tool_result.result, &run_ctx, now)?;
        }
        if default_sandbox.is_some() {
            let _ = default_sandbox;
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
            cwd: run_ctx.cwd.clone().unwrap_or_else(|| ".".to_string()),
            sandbox: run_ctx.sandbox,
            task_summary: run_ctx.task_summary.clone(),
            last_result_summary: Some(result.summary.clone()),
            status: result.status.as_str().to_string(),
            created_at_ms: run_ctx.created_at_ms,
            updated_at_ms,
        })?;
        Ok(())
    }

    async fn run_codex_turn(&self, run_ctx: SessionRunContext) -> Result<CodexToolResult, String> {
        let mut client = AppServerClient::spawn(&AppServerClientConfig {
            program: self.config.program.clone(),
        })
        .await?;
        client.initialize().await?;

        let thread_id = if let Some(thread_id) = &run_ctx.thread_id {
            let _ = client
                .request(
                    "thread/resume",
                    serde_json::json!({
                        "threadId": thread_id,
                    }),
                )
                .await?;
            thread_id.clone()
        } else {
            let response = client
                .request(
                    "thread/start",
                    serde_json::json!({
                        "cwd": run_ctx.cwd,
                        "model": self.config.model,
                        "approvalPolicy": "onRequest",
                        "ephemeral": !run_ctx.persist_session,
                    }),
                )
                .await?;
            let thread_id = response
                .pointer("/thread/id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "codex thread/start missing thread.id".to_string())?
                .to_string();
            client
                .request(
                    "thread/name/set",
                    serde_json::json!({
                        "threadId": thread_id,
                        "name": run_ctx.session_name,
                    }),
                )
                .await?;
            thread_id
        };

        if run_ctx.persist_session {
            let session_id = run_ctx
                .session_id
                .clone()
                .ok_or_else(|| "missing codex session id".to_string())?;
            self.store.upsert_session(&StoredCodexSession {
                session_id,
                thread_id: thread_id.clone(),
                session_name: run_ctx.session_name.clone(),
                cwd: run_ctx.cwd.clone().unwrap_or_else(|| ".".to_string()),
                sandbox: run_ctx.sandbox,
                task_summary: run_ctx.task_summary.clone(),
                last_result_summary: None,
                status: "running".to_string(),
                created_at_ms: run_ctx.created_at_ms,
                updated_at_ms: now_ms(),
            })?;
        }

        let turn_response = client
            .request(
                "turn/start",
                serde_json::json!({
                    "threadId": thread_id,
                    "input": build_turn_input(&run_ctx.instruction, &run_ctx.local_images),
                    "cwd": run_ctx.cwd,
                    "approvalPolicy": "onRequest",
                    "sandboxPolicy": sandbox_policy_json(run_ctx.sandbox, run_ctx.cwd.as_deref(), &run_ctx.writable_roots),
                    "model": self.config.model,
                    "outputSchema": structured_turn_output_schema(),
                }),
            )
            .await?;
        let turn_id = turn_response
            .pointer("/turn/id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "codex turn/start missing turn.id".to_string())?
            .to_string();

        let streamed = timeout(
            Duration::from_millis(self.config.turn_timeout_ms),
            self.wait_for_turn_completion(&mut client, &run_ctx, &thread_id, &turn_id),
        )
        .await
        .map_err(|_| {
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
            self.store.record_turn(
                &session_id,
                parsed.turn_id.as_deref(),
                &run_ctx.instruction,
                parsed.status.as_str(),
                &parsed.summary,
                parsed.user_visible.as_deref(),
                run_ctx.created_at_ms,
                now_ms(),
            )?;
        }

        Ok(CodexToolResult { result: parsed })
    }

    async fn wait_for_turn_completion(
        &self,
        client: &mut AppServerClient,
        run_ctx: &SessionRunContext,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<TurnCompletion, String> {
        let mut state = TurnStreamState::default();
        loop {
            let message = client.next_message().await?;
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
                        serde_json::json!({
                            "decision": approval_action_to_codex_decision(decision),
                        }),
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
                        serde_json::json!({
                            "decision": approval_action_to_codex_decision(decision),
                        }),
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
            allow_approve_always: true,
            created_at_ms: now_ms(),
        };
        self.append_runtime_event(RuntimeEvent::ApprovalRequested(requested.clone()))
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
        Ok(resolved.action)
    }

    async fn append_runtime_event(&self, event: RuntimeEvent) -> Result<(), String> {
        self.event_log
            .append(event)
            .await
            .map_err(format_event_log_error)
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
    async fn exec(&self, request: CodexExecRequest) -> Result<CodexToolResult, String> {
        self.exec_internal(request).await
    }

    async fn run_session(&self, request: CodexSessionRequest) -> Result<CodexToolResult, String> {
        self.session_internal(request).await
    }

    async fn planner_sessions(&self) -> Result<Vec<PlannerCodexSession>, String> {
        self.store.planner_sessions(12)
    }
}

struct TurnCompletion {
    final_text: String,
    turn_status: Option<String>,
    turn_error: Option<String>,
}

fn build_turn_input(instruction: &str, local_images: &[String]) -> Vec<serde_json::Value> {
    let mut input = vec![serde_json::json!({
        "type": "text",
        "text": format!(
            "{}\n\nReturn structured output only. Use `status=completed` when this turn finished the requested work, `needs_followup` when Sieve should continue the same Codex session with more instructions, and `failed` when blocked or not completed. `summary` concise factual summary. `user_visible` short user-facing text or null.",
            instruction.trim()
        ),
    })];
    for image in local_images {
        input.push(serde_json::json!({
            "type": "localImage",
            "path": image,
        }));
    }
    input
}

fn sandbox_policy_json(
    sandbox: CodexSandboxMode,
    cwd: Option<&str>,
    writable_roots: &[String],
) -> serde_json::Value {
    match sandbox {
        CodexSandboxMode::ReadOnly => serde_json::json!({
            "type": "readOnly",
            "readOnlyAccess": "fullAccess",
            "networkAccess": false,
            "excludeTmpdirEnvVar": false,
            "excludeSlashTmp": false,
        }),
        CodexSandboxMode::WorkspaceWrite => {
            let mut roots = Vec::new();
            if let Some(cwd) = cwd {
                if !cwd.trim().is_empty() {
                    roots.push(cwd.to_string());
                }
            }
            for root in writable_roots {
                if !root.trim().is_empty() && !roots.iter().any(|existing| existing == root) {
                    roots.push(root.clone());
                }
            }
            serde_json::json!({
                "type": "workspaceWrite",
                "writableRoots": roots,
                "readOnlyAccess": "fullAccess",
                "networkAccess": false,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false,
            })
        }
    }
}

fn structured_turn_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "status": {
                "type": "string",
                "enum": ["completed", "needs_followup", "failed"]
            },
            "summary": {"type": "string"},
            "user_visible": {"type": ["string", "null"]}
        },
        "required": ["status", "summary", "user_visible"]
    })
}

fn decode_turn_result(
    session_id: Option<String>,
    session_name: String,
    turn_id: Option<String>,
    thread_id: Option<String>,
    final_text: &str,
    turn_status: Option<&str>,
    turn_error: Option<&str>,
) -> CodexTurnResult {
    let fallback_status = match turn_status {
        Some("failed") | Some("interrupted") => CodexTurnStatus::Failed,
        _ => CodexTurnStatus::Completed,
    };
    let trimmed = final_text.trim();
    let mut parsed_status = None;
    let mut parsed_summary = None;
    let mut parsed_user_visible = None;
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        parsed_status = value
            .get("status")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| match value {
                "completed" => Some(CodexTurnStatus::Completed),
                "needs_followup" => Some(CodexTurnStatus::NeedsFollowup),
                "failed" => Some(CodexTurnStatus::Failed),
                _ => None,
            });
        parsed_summary = value
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        parsed_user_visible = value
            .get("user_visible")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
    }
    let summary = parsed_summary
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            turn_error
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| {
            if trimmed.is_empty() {
                "codex completed without textual output".to_string()
            } else {
                trimmed.to_string()
            }
        });
    CodexTurnResult {
        session_id,
        session_name,
        status: parsed_status.unwrap_or(fallback_status),
        summary,
        user_visible: parsed_user_visible,
        turn_id,
        thread_id,
    }
}

fn handle_item_notification(message: &serde_json::Value, state: &mut TurnStreamState) {
    let Some(item) = message.pointer("/params/item") else {
        return;
    };
    let Some(item_type) = item.get("type").and_then(serde_json::Value::as_str) else {
        return;
    };
    match item_type {
        "agentMessage" => {
            if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                state.final_agent_text = Some(text.to_string());
            }
        }
        "fileChange" => {
            let item_id = item
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let preview = file_change_preview(item);
            if !item_id.is_empty() && !preview.is_empty() {
                state
                    .file_change_previews
                    .insert(item_id.to_string(), preview);
            }
        }
        _ => {}
    }
}

fn file_change_preview(item: &serde_json::Value) -> String {
    let mut lines = Vec::new();
    if let Some(changes) = item.get("changes").and_then(serde_json::Value::as_array) {
        for change in changes {
            let path = change
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let diff = change
                .get("diff")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            lines.push(format!("path: {path}"));
            if !diff.trim().is_empty() {
                lines.push(truncate_preview(diff, 1200));
            }
        }
    }
    lines.join("\n")
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if out.chars().count() >= max_chars {
            out.push_str("\n...[truncated]");
            break;
        }
        out.push(ch);
    }
    out
}

fn approval_action_to_codex_decision(action: ApprovalAction) -> &'static str {
    match action {
        ApprovalAction::ApproveOnce => "accept",
        ApprovalAction::ApproveAlways => "acceptForSession",
        ApprovalAction::Deny => "decline",
    }
}

fn format_event_log_error(err: EventLogError) -> String {
    err.to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_runtime::{InProcessApprovalBus, RuntimeEventLog};
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use tokio::time::sleep;

    #[test]
    fn sandbox_policy_workspace_write_includes_cwd_and_roots() {
        let value = sandbox_policy_json(
            CodexSandboxMode::WorkspaceWrite,
            Some("/repo"),
            &["/repo/subdir".to_string()],
        );
        assert_eq!(
            value.get("type").and_then(serde_json::Value::as_str),
            Some("workspaceWrite")
        );
        let roots = value
            .get("writableRoots")
            .and_then(serde_json::Value::as_array)
            .expect("roots array");
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn decode_turn_result_uses_structured_payload() {
        let result = decode_turn_result(
            Some("session-1".to_string()),
            "fix-auth-flow".to_string(),
            Some("turn-1".to_string()),
            Some("thread-1".to_string()),
            r#"{"status":"needs_followup","summary":"implemented phase 1","user_visible":null}"#,
            Some("completed"),
            None,
        );
        assert_eq!(result.status, CodexTurnStatus::NeedsFollowup);
        assert_eq!(result.summary, "implemented phase 1");
        assert!(result.user_visible.is_none());
    }

    #[test]
    fn handle_item_notification_extracts_file_change_preview() {
        let mut state = TurnStreamState::default();
        handle_item_notification(
            &serde_json::json!({
                "params": {
                    "item": {
                        "type": "fileChange",
                        "id": "patch-1",
                        "changes": [
                            {
                                "path": "src/main.rs",
                                "diff": "@@ -1 +1 @@\n-old\n+new"
                            }
                        ]
                    }
                }
            }),
            &mut state,
        );
        assert!(state
            .file_change_previews
            .get("patch-1")
            .is_some_and(|value| value.contains("src/main.rs")));
    }

    #[tokio::test]
    async fn codex_manager_exec_passes_local_image_input() {
        let root = unique_test_root("codex-exec");
        let script = write_mock_server(
            &root,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id": msg["id"], "result": {"ok": True}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/start":
        assert msg["params"]["ephemeral"] is True
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_exec"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        params = msg["params"]
        assert params["sandboxPolicy"]["type"] == "readOnly"
        assert params["sandboxPolicy"]["networkAccess"] is False
        assert any(entry["type"] == "localImage" and entry["path"].endswith("image-input.png") for entry in params["input"])
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_exec"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_1", "text": "{\"status\":\"completed\",\"summary\":\"ocr ok\",\"user_visible\":\"ocr text\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_exec", "threadId": "thr_exec", "status": "completed"}}}), flush=True)
"#,
        );
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(RecordingEventLog::default());
        let manager = test_manager(&root, &script, approval_bus, event_log);

        let result = manager
            .exec(CodexExecRequest {
                instruction: "read the screenshot".to_string(),
                sandbox: CodexSandboxMode::ReadOnly,
                cwd: None,
                writable_roots: Vec::new(),
                local_images: vec![root.join("image-input.png").to_string_lossy().to_string()],
            })
            .await
            .expect("codex exec");

        assert_eq!(result.result.status, CodexTurnStatus::Completed);
        assert_eq!(result.result.user_visible.as_deref(), Some("ocr text"));
    }

    #[tokio::test]
    async fn codex_manager_persists_and_resumes_session_threads() {
        let root = unique_test_root("codex-session");
        let first_script = write_mock_server(
            &root,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id": msg["id"], "result": {"ok": True}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/start":
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_saved"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_saved_1"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_1", "text": "{\"status\":\"completed\",\"summary\":\"phase one done\",\"user_visible\":null}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_saved_1", "threadId": "thr_saved", "status": "completed"}}}), flush=True)
"#,
        );
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(RecordingEventLog::default());
        let manager = test_manager(
            &root,
            &first_script,
            approval_bus.clone(),
            event_log.clone(),
        );

        let first = manager
            .run_session(CodexSessionRequest {
                session_id: None,
                instruction: "implement phase one".to_string(),
                sandbox: CodexSandboxMode::WorkspaceWrite,
                cwd: Some("/tmp/repo".to_string()),
                writable_roots: vec!["/tmp/repo".to_string()],
                local_images: Vec::new(),
            })
            .await
            .expect("first session run");

        let session_id = first
            .result
            .session_id
            .clone()
            .expect("persistent session id");
        let stored = manager.planner_sessions().await.expect("planner sessions");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].session_id, session_id);
        assert_eq!(stored[0].status, "completed");

        let second_script = write_mock_server(
            &root,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id": msg["id"], "result": {"ok": True}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/resume":
        assert msg["params"]["threadId"] == "thr_saved"
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_saved"}}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_saved_2"}}}), flush=True)
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_2", "text": "{\"status\":\"needs_followup\",\"summary\":\"phase two started\",\"user_visible\":null}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_saved_2", "threadId": "thr_saved", "status": "completed"}}}), flush=True)
"#,
        );
        let manager = test_manager(&root, &second_script, approval_bus, event_log);
        let second = manager
            .run_session(CodexSessionRequest {
                session_id: Some(session_id.clone()),
                instruction: "continue phase two".to_string(),
                sandbox: CodexSandboxMode::WorkspaceWrite,
                cwd: Some("/tmp/repo".to_string()),
                writable_roots: vec!["/tmp/repo".to_string()],
                local_images: Vec::new(),
            })
            .await
            .expect("second session run");

        assert_eq!(second.result.status, CodexTurnStatus::NeedsFollowup);
        let stored = manager.planner_sessions().await.expect("planner sessions");
        assert_eq!(stored[0].session_id, session_id);
        assert_eq!(stored[0].status, "needs_followup");
        assert_eq!(
            stored[0].last_result_summary.as_deref(),
            Some("phase two started")
        );
    }

    #[tokio::test]
    async fn codex_manager_file_change_approval_uses_session_prefix_and_preview() {
        let root = unique_test_root("codex-approval");
        let script = write_mock_server(
            &root,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        print(json.dumps({"id": msg["id"], "result": {"ok": True}}), flush=True)
    elif method == "initialized":
        continue
    elif method == "thread/start":
        print(json.dumps({"id": msg["id"], "result": {"thread": {"id": "thr_patch"}}}), flush=True)
    elif method == "thread/name/set":
        print(json.dumps({"id": msg["id"], "result": {}}), flush=True)
    elif method == "turn/start":
        print(json.dumps({"id": msg["id"], "result": {"turn": {"id": "turn_patch"}}}), flush=True)
        print(json.dumps({"method": "item/started", "params": {"item": {"type": "fileChange", "id": "patch_1", "changes": [{"path": "src/main.rs", "diff": "@@ -1 +1 @@\\n-old\\n+new"}]}}}), flush=True)
        print(json.dumps({"id": 91, "method": "item/fileChange/requestApproval", "params": {"threadId": "thr_patch", "turnId": "turn_patch", "itemId": "patch_1", "reason": "apply patch"}}), flush=True)
        response = json.loads(sys.stdin.readline())
        assert response["result"]["decision"] == "acceptForSession"
        print(json.dumps({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "msg_patch", "text": "{\"status\":\"completed\",\"summary\":\"patch applied\",\"user_visible\":\"patch done\"}"}}}), flush=True)
        print(json.dumps({"method": "turn/completed", "params": {"turn": {"id": "turn_patch", "threadId": "thr_patch", "status": "completed"}}}), flush=True)
"#,
        );
        let approval_bus = Arc::new(InProcessApprovalBus::new());
        let event_log = Arc::new(RecordingEventLog::default());
        let manager = Arc::new(test_manager(
            &root,
            &script,
            approval_bus.clone(),
            event_log.clone(),
        ));
        let manager_task = {
            let manager = manager.clone();
            tokio::spawn(async move {
                manager
                    .run_session(CodexSessionRequest {
                        session_id: None,
                        instruction: "apply the patch".to_string(),
                        sandbox: CodexSandboxMode::WorkspaceWrite,
                        cwd: Some("/tmp/repo".to_string()),
                        writable_roots: vec!["/tmp/repo".to_string()],
                        local_images: Vec::new(),
                    })
                    .await
            })
        };

        let requested = wait_for_published_approval(&approval_bus).await;
        assert_eq!(requested.prompt_kind, ApprovalPromptKind::FileChange);
        assert!(requested
            .title
            .as_deref()
            .is_some_and(|value| value.contains("apply-patch")));
        assert!(requested
            .preview
            .as_deref()
            .is_some_and(|value| value.contains("src/main.rs")));
        approval_bus
            .resolve(ApprovalResolvedEvent {
                schema_version: 1,
                request_id: requested.request_id.clone(),
                run_id: requested.run_id.clone(),
                action: ApprovalAction::ApproveAlways,
                created_at_ms: now_ms(),
            })
            .expect("resolve approval");

        let result = manager_task
            .await
            .expect("join manager task")
            .expect("manager result");
        assert_eq!(result.result.user_visible.as_deref(), Some("patch done"));

        let events = event_log.events.lock().expect("event log lock");
        assert!(events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ApprovalRequested(_))));
        assert!(events
            .iter()
            .any(|event| matches!(event, RuntimeEvent::ApprovalResolved(_))));
    }

    #[derive(Default)]
    struct RecordingEventLog {
        events: Mutex<Vec<RuntimeEvent>>,
    }

    #[async_trait]
    impl RuntimeEventLog for RecordingEventLog {
        async fn append(&self, event: RuntimeEvent) -> Result<(), sieve_runtime::EventLogError> {
            self.events.lock().expect("event log lock").push(event);
            Ok(())
        }
    }

    fn test_manager(
        root: &PathBuf,
        script: &PathBuf,
        approval_bus: Arc<InProcessApprovalBus>,
        event_log: Arc<RecordingEventLog>,
    ) -> CodexManager {
        CodexManager {
            config: CodexManagerConfig {
                program: script.to_string_lossy().to_string(),
                model: Some("mock-model".to_string()),
                turn_timeout_ms: 5_000,
            },
            store: CodexSessionStore::new(root.join("state/codex.db")).expect("create codex store"),
            approval_bus,
            event_log,
        }
    }

    async fn wait_for_published_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
        for _ in 0..200 {
            if let Ok(events) = bus.published_events() {
                if let Some(event) = events.last() {
                    return event.clone();
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
        panic!("approval was not published");
    }

    fn write_mock_server(root: &PathBuf, body: &str) -> PathBuf {
        let path = root.join(format!("mock-codex-server-{}.py", Uuid::new_v4()));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create mock server dir");
        }
        let mut file = fs::File::create(&path).expect("create mock server");
        file.write_all(body.as_bytes()).expect("write mock server");
        let mut permissions = file.metadata().expect("mock server metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod mock server");
        path
    }

    fn unique_test_root(prefix: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).expect("create test root");
        root
    }
}
