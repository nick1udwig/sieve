use super::ComposeContext;
use crate::json_util::to_json_value;
use crate::logging::now_ms;
use serde::Serialize;
use sieve_types::PlannerGuidanceSignal;

pub(super) struct ComposeAuditRequest<'a> {
    pub(super) context: &'a ComposeContext<'a>,
    pub(super) attempts: &'a [serde_json::Value],
    pub(super) final_message: &'a str,
    pub(super) output_ref_ids: &'a [String],
    pub(super) source_urls: &'a [String],
    pub(super) quality_gate: Option<&'a str>,
    pub(super) grounding_gate: Option<&'a str>,
    pub(super) planner_followup_signal: Option<PlannerGuidanceSignal>,
}

#[derive(Serialize)]
struct ComposeAuditInputRef {
    ref_id: String,
    path: String,
}

#[derive(Serialize)]
struct ComposeAuditOutputRef {
    ref_id: String,
    path: String,
}

#[derive(Serialize)]
struct ComposeAuditRecord<'a> {
    input_refs: Vec<ComposeAuditInputRef>,
    output_ref: ComposeAuditOutputRef,
    output_ref_ids: &'a [String],
    source_urls: &'a [String],
    quality_gate: Option<&'a str>,
    grounding_gate: Option<&'a str>,
    planner_followup_signal_code: Option<u16>,
}

pub(super) async fn write_compose_audit_artifacts(
    request: ComposeAuditRequest<'_>,
) -> Result<(), String> {
    let run_id = request.context.run_id;
    let run_dir = request.context.sieve_home.join("artifacts").join(&run_id.0);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|err| format!("failed to create compose artifact dir: {err}"))?;

    let mut input_refs = Vec::new();
    for (idx, attempt) in request.attempts.iter().enumerate() {
        let ref_id = format!("assistant-compose-input:{}:{}", run_id.0, idx + 1);
        let path = run_dir.join(format!("assistant-compose-input-{}.json", idx + 1));
        let content = serde_json::to_vec_pretty(attempt)
            .map_err(|err| format!("failed to encode compose payload: {err}"))?;
        tokio::fs::write(&path, content)
            .await
            .map_err(|err| format!("failed to write compose payload artifact: {err}"))?;
        input_refs.push(ComposeAuditInputRef {
            ref_id,
            path: path.to_string_lossy().into_owned(),
        });
    }

    let output_ref_id = format!("assistant-compose-output:{}", run_id.0);
    let output_path = run_dir.join("assistant-compose-output.txt");
    tokio::fs::write(&output_path, request.final_message.as_bytes())
        .await
        .map_err(|err| format!("failed to write compose output artifact: {err}"))?;

    let record = to_json_value(
        ComposeAuditRecord {
            input_refs,
            output_ref: ComposeAuditOutputRef {
                ref_id: output_ref_id,
                path: output_path.to_string_lossy().into_owned(),
            },
            output_ref_ids: request.output_ref_ids,
            source_urls: request.source_urls,
            quality_gate: request.quality_gate,
            grounding_gate: request.grounding_gate,
            planner_followup_signal_code: request
                .planner_followup_signal
                .map(PlannerGuidanceSignal::code),
        },
        "compose audit record",
    );
    request
        .context
        .event_log
        .append_app_event("compose", "compose_audit", "info", run_id, now_ms(), record)
        .await
        .map_err(|err| format!("failed to append compose audit event: {err}"))
}
