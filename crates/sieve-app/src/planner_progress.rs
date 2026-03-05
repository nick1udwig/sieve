use crate::{
    format_integrity, mainline_artifact_kind_name, user_requested_sources,
    MainlineArtifactKind, MainlineRunReport, PlannerToolResult, RuntimeDisposition,
};
use sieve_types::PlannerGuidanceSignal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BashActionClass {
    Discovery,
    Fetch,
    Extract,
    Other,
}

impl BashActionClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Fetch => "fetch",
            Self::Extract => "extract",
            Self::Other => "other",
        }
    }
}

pub(crate) const MIN_PRIMARY_FETCH_STDOUT_BYTES: u64 = 256;

#[derive(Debug, Clone, Copy, Default)]
struct ToolProgressSummary {
    discovery_success_count: usize,
    discovery_output_count: usize,
    fetch_success_count: usize,
    non_asset_fetch_output_count: usize,
    primary_fetch_output_count: usize,
    markdown_fetch_output_count: usize,
    denied_count: usize,
}

fn first_shell_word(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

pub(crate) fn classify_bash_action(command: &str) -> BashActionClass {
    let cmd = first_shell_word(command)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match cmd.as_str() {
        "bravesearch" | "brave-search" => BashActionClass::Discovery,
        "curl" | "wget" => BashActionClass::Fetch,
        "jq" | "awk" | "sed" | "grep" | "rg" => BashActionClass::Extract,
        _ => BashActionClass::Other,
    }
}

pub(crate) fn command_targets_markdown_view(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("https://markdown.new/") || lower.contains("http://markdown.new/")
}

fn command_targets_likely_asset(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("imgs.search.brave.com")
        || lower.contains("favicon")
        || lower.contains(".png")
        || lower.contains(".jpg")
        || lower.contains(".jpeg")
        || lower.contains(".gif")
        || lower.contains(".webp")
        || lower.contains(".svg")
        || lower.contains(".ico")
        || lower.contains(".css")
        || lower.contains(".js")
}

pub(crate) fn url_is_likely_asset(url: &str) -> bool {
    command_targets_likely_asset(url)
}

fn summarize_tool_progress(tool_results: &[PlannerToolResult]) -> ToolProgressSummary {
    let mut summary = ToolProgressSummary::default();
    for result in tool_results {
        match result {
            PlannerToolResult::Bash {
                command,
                disposition,
            } => {
                let action = classify_bash_action(command);
                match disposition {
                    RuntimeDisposition::ExecuteMainline(report) => {
                        let success = report.exit_code.unwrap_or(1) == 0;
                        let stdout_bytes: u64 = report
                            .artifacts
                            .iter()
                            .filter(|artifact| {
                                matches!(artifact.kind, MainlineArtifactKind::Stdout)
                            })
                            .map(|artifact| artifact.byte_count)
                            .sum();
                        let has_output = stdout_bytes > 0;
                        if success {
                            match action {
                                BashActionClass::Discovery => {
                                    summary.discovery_success_count =
                                        summary.discovery_success_count.saturating_add(1);
                                    if has_output {
                                        summary.discovery_output_count =
                                            summary.discovery_output_count.saturating_add(1);
                                    }
                                }
                                BashActionClass::Fetch => {
                                    summary.fetch_success_count =
                                        summary.fetch_success_count.saturating_add(1);
                                    if has_output && !command_targets_likely_asset(command) {
                                        summary.non_asset_fetch_output_count = summary
                                            .non_asset_fetch_output_count
                                            .saturating_add(1);
                                        if stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES {
                                            summary.primary_fetch_output_count = summary
                                                .primary_fetch_output_count
                                                .saturating_add(1);
                                        }
                                    }
                                    if has_output && command_targets_markdown_view(command) {
                                        summary.markdown_fetch_output_count = summary
                                            .markdown_fetch_output_count
                                            .saturating_add(1);
                                    }
                                }
                                BashActionClass::Extract | BashActionClass::Other => {}
                            }
                        }
                    }
                    RuntimeDisposition::Denied { .. } => {
                        summary.denied_count = summary.denied_count.saturating_add(1);
                    }
                    RuntimeDisposition::ExecuteQuarantine(_) => {}
                }
            }
            PlannerToolResult::Endorse { .. } | PlannerToolResult::Declassify { .. } => {}
        }
    }
    summary
}

fn summarize_observed_tool_result(result: &PlannerToolResult) -> serde_json::Value {
    match result {
        PlannerToolResult::Bash {
            command,
            disposition,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                let action_class = classify_bash_action(command);
                let stdout_bytes: u64 = report
                    .artifacts
                    .iter()
                    .filter(|artifact| matches!(artifact.kind, MainlineArtifactKind::Stdout))
                    .map(|artifact| artifact.byte_count)
                    .sum();
                let stderr_bytes: u64 = report
                    .artifacts
                    .iter()
                    .filter(|artifact| matches!(artifact.kind, MainlineArtifactKind::Stderr))
                    .map(|artifact| artifact.byte_count)
                    .sum();
                serde_json::json!({
                    "tool": "bash",
                    "command_len": command.len(),
                    "action_class": action_class.as_str(),
                    "disposition": "execute_mainline",
                    "exit_code": report.exit_code,
                    "artifact_count": report.artifacts.len(),
                    "stdout_bytes": stdout_bytes,
                    "stderr_bytes": stderr_bytes,
                    "likely_has_candidate_urls": matches!(action_class, BashActionClass::Discovery) && stdout_bytes > 0,
                    "likely_has_primary_content": matches!(action_class, BashActionClass::Fetch)
                        && stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES
                        && !command_targets_likely_asset(command),
                    "uses_markdown_view": command_targets_markdown_view(command),
                    "likely_asset_target": command_targets_likely_asset(command),
                })
            }
            RuntimeDisposition::ExecuteQuarantine(report) => serde_json::json!({
                "tool": "bash",
                "command_len": command.len(),
                "action_class": classify_bash_action(command).as_str(),
                "disposition": "execute_quarantine",
                "exit_code": report.exit_code,
                "trace_path_present": !report.trace_path.trim().is_empty(),
                "stdout_path_present": report.stdout_path.as_deref().is_some(),
                "stderr_path_present": report.stderr_path.as_deref().is_some()
            }),
            RuntimeDisposition::Denied { reason } => serde_json::json!({
                "tool": "bash",
                "command_len": command.len(),
                "action_class": classify_bash_action(command).as_str(),
                "disposition": "denied",
                "reason_len": reason.len()
            }),
        },
        PlannerToolResult::Endorse {
            request,
            transition,
        } => serde_json::json!({
            "tool": "endorse",
            "value_ref_len": request.value_ref.0.len(),
            "target_integrity": format_integrity(request.target_integrity),
            "applied": transition.is_some()
        }),
        PlannerToolResult::Declassify {
            request,
            transition,
        } => serde_json::json!({
            "tool": "declassify",
            "value_ref_len": request.value_ref.0.len(),
            "sink_len": request.sink.0.len(),
            "applied": transition.is_some()
        }),
    }
}

fn normalize_bash_command_for_repeat_guard(command: &str) -> String {
    command
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn mainline_artifact_signature(report: &MainlineRunReport) -> Vec<(String, u64, u64)> {
    let mut signature = report
        .artifacts
        .iter()
        .map(|artifact| {
            (
                mainline_artifact_kind_name(artifact.kind).to_string(),
                artifact.byte_count,
                artifact.line_count,
            )
        })
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

pub(crate) fn has_repeated_bash_outcome(tool_results: &[PlannerToolResult]) -> bool {
    if tool_results.len() < 2 {
        return false;
    }

    let prev = &tool_results[tool_results.len() - 2];
    let last = &tool_results[tool_results.len() - 1];
    match (prev, last) {
        (
            PlannerToolResult::Bash {
                command: left_command,
                disposition: left_disposition,
            },
            PlannerToolResult::Bash {
                command: right_command,
                disposition: right_disposition,
            },
        ) if normalize_bash_command_for_repeat_guard(left_command)
            == normalize_bash_command_for_repeat_guard(right_command) =>
        {
            match (left_disposition, right_disposition) {
                (
                    RuntimeDisposition::ExecuteMainline(left_report),
                    RuntimeDisposition::ExecuteMainline(right_report),
                ) => {
                    left_report.exit_code == right_report.exit_code
                        && mainline_artifact_signature(left_report)
                            == mainline_artifact_signature(right_report)
                }
                (
                    RuntimeDisposition::Denied {
                        reason: left_reason,
                    },
                    RuntimeDisposition::Denied {
                        reason: right_reason,
                    },
                ) => left_reason == right_reason,
                _ => false,
            }
        }
        _ => false,
    }
}

pub(crate) fn build_guidance_prompt(
    trusted_user_message: &str,
    step_index: usize,
    max_steps: usize,
    step_results: &[PlannerToolResult],
    all_results: &[PlannerToolResult],
) -> String {
    let observed_results: Vec<serde_json::Value> = step_results
        .iter()
        .map(summarize_observed_tool_result)
        .collect();
    let step_progress = summarize_tool_progress(step_results);
    let total_progress = summarize_tool_progress(all_results);
    serde_json::json!({
        "task": "planner_act_observe",
        "trusted_user_message": trusted_user_message,
        "step_index": step_index,
        "max_steps": max_steps,
        "step_tool_result_count": step_results.len(),
        "total_tool_result_count": all_results.len(),
        "step_progress": {
            "discovery_success_count": step_progress.discovery_success_count,
            "discovery_output_count": step_progress.discovery_output_count,
            "fetch_success_count": step_progress.fetch_success_count,
            "non_asset_fetch_output_count": step_progress.non_asset_fetch_output_count,
            "primary_fetch_output_count": step_progress.primary_fetch_output_count,
            "markdown_fetch_output_count": step_progress.markdown_fetch_output_count,
            "denied_count": step_progress.denied_count,
        },
        "total_progress": {
            "discovery_success_count": total_progress.discovery_success_count,
            "discovery_output_count": total_progress.discovery_output_count,
            "fetch_success_count": total_progress.fetch_success_count,
            "non_asset_fetch_output_count": total_progress.non_asset_fetch_output_count,
            "primary_fetch_output_count": total_progress.primary_fetch_output_count,
            "markdown_fetch_output_count": total_progress.markdown_fetch_output_count,
            "denied_count": total_progress.denied_count,
            "has_repeated_no_gain": has_repeated_bash_outcome(all_results),
        },
        "observed_step_results": observed_results,
        "instruction": "Return numeric guidance code: continue only if more tool actions are still needed; otherwise return final or stop. When discovery output exists but non-asset fetch content is still missing, prefer continue code 110 before finalizing."
    })
    .to_string()
}

pub(crate) fn guidance_requests_continue(signal: PlannerGuidanceSignal) -> bool {
    matches!(
        signal,
        PlannerGuidanceSignal::ContinueNeedEvidence
            | PlannerGuidanceSignal::ContinueFetchPrimarySource
            | PlannerGuidanceSignal::ContinueFetchAdditionalSource
            | PlannerGuidanceSignal::ContinueRefineApproach
            | PlannerGuidanceSignal::ContinueNeedRequiredParameter
            | PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence
            | PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint
            | PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool
            | PlannerGuidanceSignal::ContinueNeedHigherQualitySource
            | PlannerGuidanceSignal::ContinueResolveSourceConflict
            | PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch
            | PlannerGuidanceSignal::ContinueNeedUrlExtraction
            | PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl
            | PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction
    )
}

pub(crate) fn guidance_continue_decision(
    signal: PlannerGuidanceSignal,
    consecutive_empty_steps: usize,
    planner_steps_taken: usize,
    planner_step_limit: usize,
    planner_step_hard_limit: usize,
) -> (bool, usize, bool) {
    let mut auto_extended_limit = false;
    let mut should_continue = guidance_requests_continue(signal) && consecutive_empty_steps < 2;
    let mut effective_step_limit = planner_step_limit;
    if should_continue && planner_steps_taken >= effective_step_limit {
        if effective_step_limit < planner_step_hard_limit {
            effective_step_limit = effective_step_limit.saturating_add(1);
            auto_extended_limit = true;
        } else {
            should_continue = false;
        }
    }
    (should_continue, effective_step_limit, auto_extended_limit)
}

fn signal_claims_fact_ready(signal: PlannerGuidanceSignal) -> bool {
    matches!(
        signal,
        PlannerGuidanceSignal::FinalAnswerReady
            | PlannerGuidanceSignal::FinalAnswerPartial
            | PlannerGuidanceSignal::FinalSingleFactReady
            | PlannerGuidanceSignal::FinalConflictingFactsWithRange
    )
}

fn signal_is_hard_stop(signal: PlannerGuidanceSignal) -> bool {
    matches!(
        signal,
        PlannerGuidanceSignal::StopPolicyBlocked
            | PlannerGuidanceSignal::StopBudgetExhausted
            | PlannerGuidanceSignal::StopNoAllowedToolCanSatisfyTask
            | PlannerGuidanceSignal::ErrorContractViolation
    )
}

pub(crate) fn progress_contract_override_signal(
    trusted_user_message: &str,
    signal: PlannerGuidanceSignal,
    tool_results: &[PlannerToolResult],
) -> Option<(PlannerGuidanceSignal, &'static str)> {
    if user_requested_sources(trusted_user_message) || signal_is_hard_stop(signal) {
        return None;
    }
    let progress = summarize_tool_progress(tool_results);
    if progress.discovery_output_count == 0 {
        return None;
    }
    if progress.primary_fetch_output_count > 0 {
        return None;
    }
    if progress.non_asset_fetch_output_count > 0 {
        if signal == PlannerGuidanceSignal::ContinueNeedHigherQualitySource {
            return None;
        }
        return Some((
            PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
            "fetch_output_low_signal",
        ));
    }
    if progress.fetch_success_count > 0 {
        if signal == PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl {
            return None;
        }
        return Some((
            PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl,
            "missing_non_asset_fetch_content",
        ));
    }
    if has_repeated_bash_outcome(tool_results) {
        if signal == PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction {
            return None;
        }
        return Some((
            PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction,
            "repeated_no_progress",
        ));
    }
    if signal == PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch {
        return None;
    }
    if !guidance_requests_continue(signal) && !signal_claims_fact_ready(signal) {
        return None;
    }
    Some((
        PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch,
        "missing_primary_content_fetch",
    ))
}
