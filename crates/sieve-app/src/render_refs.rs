use sieve_llm::{SummaryModel, SummaryRequest};
use sieve_types::RunId;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) enum RenderRef {
    Literal {
        value: String,
    },
    Artifact {
        path: PathBuf,
        byte_count: u64,
        line_count: u64,
    },
}

pub(crate) async fn render_assistant_message(
    message: &str,
    referenced_ref_ids: &BTreeSet<String>,
    summarized_ref_ids: &BTreeSet<String>,
    render_refs: &BTreeMap<String, RenderRef>,
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
) -> String {
    let mut expanded = message.to_string();

    for ref_id in referenced_ref_ids {
        if let Some(raw_value) = resolve_raw_ref_value(ref_id, render_refs).await {
            let token = format!("[[ref:{ref_id}]]");
            expanded = expanded.replace(&token, &raw_value);
        }
    }

    for ref_id in summarized_ref_ids {
        let token = format!("[[summary:{ref_id}]]");
        if !expanded.contains(&token) {
            continue;
        }
        if let Some((content, byte_count, line_count)) =
            resolve_ref_summary_input(ref_id, render_refs).await
        {
            let summary = match summary_model
                .summarize_ref(SummaryRequest {
                    run_id: run_id.clone(),
                    ref_id: ref_id.clone(),
                    content,
                    byte_count,
                    line_count,
                })
                .await
            {
                Ok(summary) => summary,
                Err(err) => format!("summary unavailable: {err}"),
            };
            expanded = expanded.replace(&token, &summary);
        }
    }

    expanded
}

async fn resolve_raw_ref_value(
    ref_id: &str,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Option<String> {
    let render_ref = render_refs.get(ref_id)?;
    match render_ref {
        RenderRef::Literal { value } => Some(value.clone()),
        RenderRef::Artifact { path, .. } => read_artifact_as_string(path).await.ok(),
    }
}

pub(crate) async fn resolve_ref_summary_input(
    ref_id: &str,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Option<(String, u64, u64)> {
    let render_ref = render_refs.get(ref_id)?;
    match render_ref {
        RenderRef::Literal { value } => Some((value.clone(), value.len() as u64, 0)),
        RenderRef::Artifact {
            path,
            byte_count,
            line_count,
        } => {
            let content = read_artifact_as_string(path).await.ok()?;
            Some((content, *byte_count, *line_count))
        }
    }
}

pub(crate) async fn read_artifact_as_string(path: &std::path::Path) -> Result<String, io::Error> {
    let bytes = tokio::fs::read(path).await?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}
