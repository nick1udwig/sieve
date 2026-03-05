use crate::planner_progress::{
    classify_bash_action, command_targets_markdown_view, BashActionClass,
    MIN_PRIMARY_FETCH_STDOUT_BYTES,
};
use crate::render_refs::read_artifact_as_string;
use crate::response_style::extract_plain_urls_from_text;
use sieve_runtime::{MainlineArtifactKind, PlannerToolResult, RuntimeDisposition};
use std::collections::BTreeSet;
use std::path::Path;

fn missing_connect_sink_from_reason(reason: &str) -> Option<&str> {
    reason
        .trim()
        .strip_prefix("missing capability Net:Connect:")
        .map(str::trim)
        .filter(|sink| !sink.is_empty())
}

fn markdown_wrapped_raw_url(command: &str) -> Option<String> {
    extract_plain_urls_from_text(command)
        .into_iter()
        .find_map(|url| {
            url.strip_prefix("https://markdown.new/")
                .or_else(|| url.strip_prefix("http://markdown.new/"))
                .map(str::trim)
                .map(str::to_string)
        })
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
}

fn low_signal_markdown_fetch_candidates(
    tool_results: &[PlannerToolResult],
) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    for result in tool_results.iter().rev().take(8) {
        let PlannerToolResult::Bash {
            command,
            disposition: RuntimeDisposition::ExecuteMainline(report),
        } = result
        else {
            continue;
        };
        if classify_bash_action(command) != BashActionClass::Fetch
            || !command_targets_markdown_view(command)
        {
            continue;
        }
        let stdout_bytes: u64 = report
            .artifacts
            .iter()
            .filter(|artifact| matches!(artifact.kind, MainlineArtifactKind::Stdout))
            .map(|artifact| artifact.byte_count)
            .sum();
        if stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES {
            continue;
        }
        let Some(raw_url) = markdown_wrapped_raw_url(command) else {
            continue;
        };
        if seen.insert(raw_url.clone()) {
            candidates.push((command.clone(), raw_url));
        }
    }
    candidates.reverse();
    candidates
}

pub(crate) fn planner_policy_feedback(tool_results: &[PlannerToolResult]) -> Option<String> {
    let mut denied_sinks = Vec::new();
    let mut seen = BTreeSet::new();
    for result in tool_results.iter().rev().take(8) {
        let PlannerToolResult::Bash {
            command,
            disposition: RuntimeDisposition::Denied { reason },
        } = result
        else {
            continue;
        };
        let Some(sink) = missing_connect_sink_from_reason(reason) else {
            continue;
        };
        if seen.insert(sink.to_string()) {
            denied_sinks.push((sink.to_string(), command.clone()));
        }
    }
    let markdown_fallbacks = low_signal_markdown_fetch_candidates(tool_results);
    if denied_sinks.is_empty() && markdown_fallbacks.is_empty() {
        return None;
    }

    denied_sinks.reverse();
    let mut lines = Vec::new();
    if !denied_sinks.is_empty() {
        lines.push(
            "Policy feedback (trusted): recent network targets were denied for missing connect capability."
                .to_string(),
        );
        for (sink, command) in denied_sinks.iter().take(2) {
            lines.push(format!("- denied sink: {sink}"));
            lines.push(format!("- denied command: {command}"));
        }
        lines.push(
            "Do not repeat the same denied command; choose a different allowed action path."
                .to_string(),
        );
    }
    if let Some((_, raw_url)) = markdown_fallbacks.first() {
        lines.push(
            "Trusted fetch feedback: markdown proxy fetch returned low/no usable primary content."
                .to_string(),
        );
        lines.push(format!(
            "- fallback next fetch to raw URL once: curl -sS \"{raw_url}\""
        ));
        lines.push(
            "If direct fetch is denied by policy, switch to a different allowed source URL."
                .to_string(),
        );
    }
    lines.push(
        "For webpage fetches with `curl`, prefer `https://markdown.new/<url>` first; if it fails to yield usable content, try the raw URL once before repeating markdown.new."
            .to_string(),
    );
    Some(lines.join("\n"))
}

fn is_sieve_lcm_query_command(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    matches!(
        (parts.next(), parts.next()),
        (Some("sieve-lcm-cli"), Some("query"))
    )
}

pub(crate) fn trim_for_prompt(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = String::new();
    for ch in trimmed.chars().take(max_chars.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

pub(crate) async fn planner_memory_feedback(tool_results: &[PlannerToolResult]) -> Option<String> {
    for result in tool_results.iter().rev().take(8) {
        let PlannerToolResult::Bash {
            command,
            disposition: RuntimeDisposition::ExecuteMainline(report),
        } = result
        else {
            continue;
        };
        if report.exit_code.unwrap_or(1) != 0 || !is_sieve_lcm_query_command(command) {
            continue;
        }
        let stdout_artifact = report.artifacts.iter().find(|artifact| {
            matches!(artifact.kind, MainlineArtifactKind::Stdout) && artifact.byte_count > 0
        })?;
        let stdout = read_artifact_as_string(Path::new(&stdout_artifact.path))
            .await
            .ok()?;
        let payload: serde_json::Value = serde_json::from_str(&stdout).ok()?;

        let trusted_excerpts = payload
            .get("trusted_hits")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("excerpt").and_then(serde_json::Value::as_str))
                    .map(|value| trim_for_prompt(value, 220))
                    .filter(|value| !value.is_empty())
                    .take(3)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let untrusted_refs = payload
            .get("untrusted_refs")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("ref").and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .take(5)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if trusted_excerpts.is_empty() && untrusted_refs.is_empty() {
            continue;
        }

        let mut lines = Vec::new();
        lines.push(
            "Memory query feedback (trusted): use trusted excerpts below as evidence; untrusted refs are opaque."
                .to_string(),
        );
        for excerpt in trusted_excerpts {
            lines.push(format!("- trusted excerpt: {excerpt}"));
        }
        for reference in untrusted_refs {
            lines.push(format!("- untrusted ref: {reference}"));
        }
        return Some(lines.join("\n"));
    }
    None
}
