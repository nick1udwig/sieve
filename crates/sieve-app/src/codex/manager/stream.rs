use super::super::client::AppServerClient;
use super::protocol::*;
use super::{CodexManager, SessionRunContext, SteerCommand};
use crate::json_util::to_json_value;
use sieve_types::{ApprovalPromptKind, CodexSessionLifecycleStatus, CommandSegment};
use tokio::sync::mpsc;

impl CodexManager {
    pub(super) async fn wait_for_turn_completion(
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

    pub(super) async fn request_turn_steer(
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

    pub(super) async fn handle_server_request(
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
}
