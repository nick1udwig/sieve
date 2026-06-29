use serde::Serialize;
use sieve_runtime::EventLogError;
use sieve_types::{
    ApprovalAction, CodexExecRequest, CodexExecResult, CodexSandboxMode,
    CodexSessionLifecycleStatus, CodexSessionRequest, CodexTurnResult, CodexTurnStatus,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub(super) struct CommandExecParams<'a> {
    pub(super) command: &'a [String],
    pub(super) cwd: Option<&'a str>,
    #[serde(rename = "timeoutMs")]
    pub(super) timeout_ms: Option<i64>,
    #[serde(rename = "sandboxPolicy")]
    pub(super) sandbox_policy: SandboxPolicy,
}

#[derive(Serialize)]
pub(super) struct ThreadResumeParams<'a> {
    #[serde(rename = "threadId")]
    pub(super) thread_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct ThreadStartParams<'a> {
    pub(super) cwd: Option<&'a str>,
    pub(super) model: &'a Option<String>,
    #[serde(rename = "approvalPolicy")]
    pub(super) approval_policy: &'static str,
    pub(super) ephemeral: bool,
}

#[derive(Serialize)]
pub(super) struct ThreadNameSetParams<'a> {
    #[serde(rename = "threadId")]
    pub(super) thread_id: &'a str,
    pub(super) name: &'a str,
}

#[derive(Serialize)]
pub(super) struct TurnStartParams<'a> {
    #[serde(rename = "threadId")]
    pub(super) thread_id: &'a str,
    pub(super) input: Vec<TurnInputItem>,
    pub(super) cwd: Option<&'a str>,
    #[serde(rename = "approvalPolicy")]
    pub(super) approval_policy: &'static str,
    #[serde(rename = "sandboxPolicy")]
    pub(super) sandbox_policy: SandboxPolicy,
    pub(super) model: &'a Option<String>,
    #[serde(rename = "outputSchema")]
    pub(super) output_schema: serde_json::Value,
}

#[derive(Serialize)]
pub(super) struct TurnSteerParams<'a> {
    #[serde(rename = "threadId")]
    pub(super) thread_id: &'a str,
    pub(super) input: Vec<TurnInputItem>,
    #[serde(rename = "expectedTurnId")]
    pub(super) expected_turn_id: &'a str,
}

#[derive(Serialize)]
pub(super) struct ApprovalDecision {
    pub(super) decision: &'static str,
}

#[derive(Serialize)]
pub(super) struct TurnInputItem {
    #[serde(rename = "type")]
    pub(super) item_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
pub(super) enum SandboxPolicy {
    #[serde(rename = "readOnly")]
    ReadOnly {
        #[serde(rename = "networkAccess")]
        network_access: bool,
    },
    #[serde(rename = "workspaceWrite")]
    WorkspaceWrite {
        #[serde(rename = "writableRoots")]
        writable_roots: Vec<String>,
        #[serde(rename = "networkAccess")]
        network_access: bool,
        #[serde(rename = "excludeTmpdirEnvVar")]
        exclude_tmpdir_env_var: bool,
        #[serde(rename = "excludeSlashTmp")]
        exclude_slash_tmp: bool,
    },
}

#[derive(Debug, Default)]
pub(super) struct TurnStreamState {
    pub(super) agent_delta: String,
    pub(super) final_agent_text: Option<String>,
    pub(super) file_change_previews: HashMap<String, String>,
}

pub(super) struct TurnCompletion {
    pub(super) final_text: String,
    pub(super) turn_status: Option<String>,
    pub(super) turn_error: Option<String>,
}

pub(super) fn build_turn_input(instruction: &str, local_images: &[String]) -> Vec<TurnInputItem> {
    let mut input = vec![TurnInputItem {
        item_type: "text",
        text: Some(format!(
            "{}\n\nWork in one bounded increment, not an endless first pass. After one coherent milestone or a few minutes of work, stop and return structured output. Use `status=completed` when this turn finished the requested work, `needs_followup` when Sieve should continue the same Codex session with more instructions, and `failed` when blocked or not completed. If the task is large, prefer `needs_followup` after making concrete progress instead of trying to finish everything in one turn. `summary` concise factual summary. `user_visible` short user-facing text or null.",
            instruction.trim()
        )),
        path: None,
    }];
    for image in local_images {
        input.push(TurnInputItem {
            item_type: "localImage",
            text: None,
            path: Some(image.clone()),
        });
    }
    input
}

pub(super) fn normalize_exec_request(
    request: CodexExecRequest,
) -> Result<CodexExecRequest, String> {
    let cwd = normalize_optional_path(request.cwd, None)?;
    let writable_roots = normalize_path_list(request.writable_roots, cwd.as_deref())?;
    Ok(CodexExecRequest {
        command: request.command,
        sandbox: request.sandbox,
        cwd,
        writable_roots,
        timeout_ms: request.timeout_ms,
    })
}

pub(super) fn normalize_session_request(
    request: CodexSessionRequest,
) -> Result<CodexSessionRequest, String> {
    let cwd = normalize_optional_path(request.cwd, None)?;
    let writable_roots = normalize_path_list(request.writable_roots, cwd.as_deref())?;
    let local_images = normalize_path_list(request.local_images, cwd.as_deref())?;
    Ok(CodexSessionRequest {
        session_id: request.session_id,
        instruction: request.instruction,
        sandbox: request.sandbox,
        cwd,
        writable_roots,
        local_images,
    })
}

pub(super) fn usable_stored_cwd(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Path::new(trimmed).exists().then(|| trimmed.to_string())
}

fn normalize_optional_path(
    raw: Option<String>,
    base: Option<&str>,
) -> Result<Option<String>, String> {
    raw.map(|value| normalize_path(value, base)).transpose()
}

fn normalize_path_list(paths: Vec<String>, base: Option<&str>) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let normalized = normalize_path(path, base)?;
        if !out.iter().any(|existing| existing == &normalized) {
            out.push(normalized);
        }
    }
    Ok(out)
}

fn normalize_path(raw: String, base: Option<&str>) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("codex path must be non-empty".to_string());
    }
    let expanded = if trimmed == "~" {
        home_dir()?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else {
        let path = PathBuf::from(trimmed);
        if path.is_absolute() {
            path
        } else if let Some(base) = base {
            PathBuf::from(base).join(path)
        } else {
            std::env::current_dir()
                .map_err(|err| format!("resolve current directory failed: {err}"))?
                .join(path)
        }
    };
    Ok(expanded.to_string_lossy().to_string())
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "HOME is not set; cannot expand `~` path".to_string())
}

pub(super) fn sandbox_policy_json(
    sandbox: CodexSandboxMode,
    cwd: Option<&str>,
    writable_roots: &[String],
) -> SandboxPolicy {
    match sandbox {
        CodexSandboxMode::ReadOnly => SandboxPolicy::ReadOnly {
            network_access: false,
        },
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
            SandboxPolicy::WorkspaceWrite {
                writable_roots: roots,
                network_access: false,
                exclude_tmpdir_env_var: false,
                exclude_slash_tmp: false,
            }
        }
    }
}

pub(super) fn structured_turn_output_schema() -> serde_json::Value {
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

pub(super) fn lifecycle_status_from_turn_status(
    status: CodexTurnStatus,
) -> CodexSessionLifecycleStatus {
    match status {
        CodexTurnStatus::Completed => CodexSessionLifecycleStatus::Completed,
        CodexTurnStatus::NeedsFollowup => CodexSessionLifecycleStatus::NeedsFollowup,
        CodexTurnStatus::Failed => CodexSessionLifecycleStatus::Failed,
    }
}

pub(super) fn decode_command_exec_result(
    response: &serde_json::Value,
) -> Result<CodexExecResult, String> {
    let exit_code = response
        .get("exitCode")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| "codex command/exec missing exitCode".to_string())
        .and_then(|value| {
            i32::try_from(value)
                .map_err(|_| "codex command/exec exitCode exceeds i32 range".to_string())
        })?;
    let stdout = response
        .get("stdout")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "codex command/exec missing stdout".to_string())?
        .to_string();
    let stderr = response
        .get("stderr")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "codex command/exec missing stderr".to_string())?
        .to_string();
    Ok(CodexExecResult {
        exit_code,
        stdout,
        stderr,
    })
}

pub(super) fn decode_turn_result(
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

pub(super) fn handle_item_notification(message: &serde_json::Value, state: &mut TurnStreamState) {
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

pub(super) fn truncate_preview(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("\n...[truncated]");
            break;
        }
        out.push(ch);
    }
    out
}

pub(super) fn is_missing_rollout_error(err: &str) -> bool {
    err.contains("no rollout found for thread id")
}

pub(super) fn approval_action_to_codex_decision(action: ApprovalAction) -> &'static str {
    match action {
        ApprovalAction::ApproveOnce => "accept",
        ApprovalAction::ApproveAlways => "acceptForSession",
        ApprovalAction::Deny => "decline",
    }
}

pub(super) fn format_event_log_error(err: EventLogError) -> String {
    err.to_string()
}
