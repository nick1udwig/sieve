use super::protocol::format_event_log_error;
use super::{CodexManager, SessionRunContext};
use crate::logging::now_ms;
use sieve_types::{
    ApprovalAction, ApprovalPromptKind, ApprovalRequestId, ApprovalRequestedEvent,
    ApprovalResolvedEvent, CodexSessionLifecycleStatus, CodexSessionStatusEvent, CommandSegment,
    RunId, RuntimeEvent,
};
use uuid::Uuid;

impl CodexManager {
    pub(super) async fn request_user_approval(
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

    pub(super) async fn emit_status_event(
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

    pub(super) async fn append_runtime_event(&self, event: RuntimeEvent) -> Result<(), String> {
        self.event_log
            .append(event)
            .await
            .map_err(format_event_log_error)
    }

    pub(super) async fn record_session_event(
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
