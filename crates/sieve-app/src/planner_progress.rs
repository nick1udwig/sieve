use crate::response_style::user_requested_sources;
use crate::turn::{format_integrity, mainline_artifact_kind_name};
use sieve_runtime::{
    MainlineArtifactKind, MainlineRunReport, PlannerToolResult, RuntimeDisposition,
};
use sieve_types::PlannerGuidanceSignal;
use std::fs;

const GUIDANCE_INSTRUCTION_PROMPT: &str = include_str!("prompts/guidance_instruction.md");

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
const MAX_GUIDANCE_ARTIFACT_EXCERPT_BYTES: usize = 1_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserActionClass {
    Navigate,
    Inspect,
    Interact,
    Extract,
    Other,
}

impl BrowserActionClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::Inspect => "inspect",
            Self::Interact => "interact",
            Self::Extract => "extract",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
struct AgentBrowserObservation {
    action_class: BrowserActionClass,
    session_name: Option<String>,
    target_url: Option<String>,
}

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

fn shell_basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
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

fn find_flag_value(tokens: &[&str], flag: &str) -> Option<String> {
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i];
        if token == flag {
            return tokens.get(i + 1).map(|value| (*value).to_string());
        }
        if let Some(value) = token.strip_prefix(&format!("{flag}=")) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        i += 1;
    }
    None
}

fn parse_agent_browser_observation(command: &str) -> Option<AgentBrowserObservation> {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    if shell_basename(tokens.first().copied().unwrap_or_default()) != "agent-browser" {
        return None;
    }

    let session_name = find_flag_value(&tokens, "--session");
    let action_class = match (tokens.get(1).copied(), tokens.get(2).copied()) {
        (Some("open" | "goto" | "navigate"), _) => BrowserActionClass::Navigate,
        (Some("tab"), Some("new")) => BrowserActionClass::Navigate,
        (Some("record"), Some("start" | "restart")) => BrowserActionClass::Navigate,
        (Some("snapshot"), _) => BrowserActionClass::Inspect,
        (Some("get" | "is" | "screenshot" | "pdf" | "download"), _) => BrowserActionClass::Extract,
        (
            Some(
                "click" | "dblclick" | "hover" | "focus" | "check" | "uncheck" | "select" | "drag"
                | "scroll" | "scrollintoview" | "scrollinto" | "mouse" | "fill" | "type"
                | "keyboard" | "upload" | "storage",
            ),
            _,
        ) => BrowserActionClass::Interact,
        _ => BrowserActionClass::Other,
    };

    let target_url = match (tokens.get(1).copied(), tokens.get(2).copied()) {
        (Some("open" | "goto" | "navigate"), _) => tokens.get(2).map(|value| (*value).to_string()),
        (Some("tab"), Some("new")) => tokens.get(3).map(|value| (*value).to_string()),
        (Some("record"), Some("start" | "restart")) => {
            tokens.get(4).map(|value| (*value).to_string())
        }
        _ => None,
    };

    Some(AgentBrowserObservation {
        action_class,
        session_name,
        target_url,
    })
}

fn strip_ansi_escape_sequences(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if ch != '\0' {
            out.push(ch);
        }
    }
    out
}

fn read_artifact_excerpt(path: &str) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let excerpt =
        String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_GUIDANCE_ARTIFACT_EXCERPT_BYTES)])
            .to_string();
    let cleaned = strip_ansi_escape_sequences(&excerpt).trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn is_plain_url_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

fn infer_interstitial_kind(excerpt: &str) -> Option<&'static str> {
    let lower = excerpt.to_ascii_lowercase();
    if lower.contains("google.com/sorry")
        || lower.contains("sorry/index")
        || lower.contains("unusual traffic")
        || lower.contains("captcha")
    {
        return Some("anti_bot");
    }
    if lower.contains("sign in") || lower.contains("log in") || lower.contains("login") {
        return Some("login");
    }
    if lower.contains("before you continue") || lower.contains("consent") {
        return Some("consent");
    }
    if lower.contains("age-restricted") || lower.contains("confirm your age") {
        return Some("age_gate");
    }
    if lower.contains("subscribe to continue")
        || lower.contains("membership required")
        || lower.contains("paywall")
    {
        return Some("paywall");
    }
    None
}

fn excerpt_has_result_item_markers(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    lower.contains("/watch?v=")
        || lower.contains("/shorts/")
        || lower.contains("- heading \"")
        || lower.contains("[level=3]")
        || (lower.contains("- main:") && lower.contains("- /url: /@"))
}

fn browser_target_looks_like_search(target_url: Option<&str>) -> bool {
    let Some(target_url) = target_url else {
        return false;
    };
    let lower = target_url.to_ascii_lowercase();
    lower.contains("/results?")
        || lower.contains("search_query=")
        || lower.contains("tbm=vid")
        || lower.contains("/search?")
}

fn infer_browser_page_state(
    observation: &AgentBrowserObservation,
    excerpt: Option<&str>,
) -> &'static str {
    let Some(excerpt) = excerpt else {
        return "empty";
    };
    let has_result_items = excerpt_has_result_item_markers(excerpt);
    if has_result_items {
        return if browser_target_looks_like_search(observation.target_url.as_deref()) {
            "result_list"
        } else if matches!(
            observation.action_class,
            BrowserActionClass::Inspect | BrowserActionClass::Extract
        ) {
            "answer_item"
        } else {
            "detail_page"
        };
    }
    if let Some(kind) = infer_interstitial_kind(excerpt) {
        return if kind == "anti_bot" {
            "block_page"
        } else {
            "interstitial"
        };
    }

    let nonempty_lines = excerpt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if nonempty_lines.is_empty() {
        return "empty";
    }
    if nonempty_lines.iter().all(|line| is_plain_url_line(line)) {
        return "url_only";
    }
    if nonempty_lines.len() <= 2 && nonempty_lines.iter().any(|line| is_plain_url_line(line)) {
        return "title_only";
    }
    if browser_target_looks_like_search(observation.target_url.as_deref()) {
        return "result_list";
    }
    if matches!(
        observation.action_class,
        BrowserActionClass::Inspect | BrowserActionClass::Extract
    ) {
        return "answer_item";
    }
    "detail_page"
}

fn command_failure_kind_from_reason(reason: &str) -> &'static str {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("missing capability") {
        "missing_capability"
    } else if lower.contains("uncertain command") || lower.contains("unsupported") {
        "uncertain_shape"
    } else if lower.contains("denied by mode")
        || lower.contains("blocked")
        || lower.contains("not allowed")
    {
        "blocked_mode"
    } else {
        "runtime_failure"
    }
}

fn artifact_excerpt_payloads(report: &MainlineRunReport) -> Vec<serde_json::Value> {
    report
        .artifacts
        .iter()
        .filter(|artifact| artifact.byte_count > 0)
        .take(2)
        .map(|artifact| {
            serde_json::json!({
                "ref_id": artifact.ref_id.clone(),
                "kind": mainline_artifact_kind_name(artifact.kind),
                "byte_count": artifact.byte_count,
                "line_count": artifact.line_count,
                "excerpt": read_artifact_excerpt(&artifact.path),
            })
        })
        .collect()
}

fn summarize_tool_progress(tool_results: &[PlannerToolResult]) -> ToolProgressSummary {
    let mut summary = ToolProgressSummary::default();
    for result in tool_results {
        match result {
            PlannerToolResult::Automation { .. } => {}
            PlannerToolResult::CodexExec { .. } | PlannerToolResult::CodexSession { .. } => {}
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
                                        summary.non_asset_fetch_output_count =
                                            summary.non_asset_fetch_output_count.saturating_add(1);
                                        if stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES {
                                            summary.primary_fetch_output_count = summary
                                                .primary_fetch_output_count
                                                .saturating_add(1);
                                        }
                                    }
                                    if has_output && command_targets_markdown_view(command) {
                                        summary.markdown_fetch_output_count =
                                            summary.markdown_fetch_output_count.saturating_add(1);
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
        PlannerToolResult::Automation {
            request,
            message,
            failure_reason,
            ..
        } => serde_json::json!({
            "tool": "automation",
            "action": request.action.as_str(),
            "target": request.target.as_ref().map(|value| value.as_str()),
            "schedule_kind": request.schedule.as_ref().map(|value| value.kind_str()),
            "has_schedule": request.schedule.is_some(),
            "has_prompt": request.prompt.as_ref().map(|value| !value.trim().is_empty()).unwrap_or(false),
            "has_job_id": request.job_id.as_ref().map(|value| !value.trim().is_empty()).unwrap_or(false),
            "disposition": if failure_reason.is_some() { "failed" } else { "succeeded" },
            "message_len": message.as_ref().map(|value| value.len()).unwrap_or(0),
            "failure_reason_len": failure_reason.as_ref().map(|value| value.len()).unwrap_or(0),
            "command_failure_kind": failure_reason.as_ref().map(|_| "invalid_arguments"),
        }),
        PlannerToolResult::Bash {
            command,
            disposition,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => {
                let action_class = classify_bash_action(command);
                let browser = parse_agent_browser_observation(command);
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
                let stdout_excerpt = report
                    .artifacts
                    .iter()
                    .find(|artifact| {
                        matches!(artifact.kind, MainlineArtifactKind::Stdout)
                            && artifact.byte_count > 0
                    })
                    .and_then(|artifact| read_artifact_excerpt(&artifact.path));
                let browser_observation = browser.as_ref().map(|browser| {
                    let page_state = infer_browser_page_state(browser, stdout_excerpt.as_deref());
                    let interstitial_kind = matches!(page_state, "interstitial" | "block_page")
                        .then(|| stdout_excerpt.as_deref().and_then(infer_interstitial_kind))
                        .flatten();
                    serde_json::json!({
                        "action_class": browser.action_class.as_str(),
                        "session_name": browser.session_name.clone(),
                        "session_reusable": true,
                        "target_url": browser.target_url.clone(),
                        "page_state": page_state,
                        "interstitial_kind": interstitial_kind,
                    })
                });
                let failure_kind = if report.exit_code.unwrap_or(1) != 0 {
                    Some("runtime_failure")
                } else if stdout_bytes == 0 && stderr_bytes == 0 {
                    Some("empty_output")
                } else {
                    None
                };
                serde_json::json!({
                    "tool": "bash",
                    "command_len": command.len(),
                    "action_class": action_class.as_str(),
                    "browser_action_class": browser.as_ref().map(|browser| browser.action_class.as_str()),
                    "disposition": "execute_mainline",
                    "exit_code": report.exit_code,
                    "artifact_count": report.artifacts.len(),
                    "stdout_bytes": stdout_bytes,
                    "stderr_bytes": stderr_bytes,
                    "raw_artifacts": artifact_excerpt_payloads(report),
                    "command_failure_kind": failure_kind,
                    "browser_observation": browser_observation,
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
                "browser_action_class": parse_agent_browser_observation(command)
                    .map(|browser| browser.action_class.as_str()),
                "disposition": "execute_quarantine",
                "exit_code": report.exit_code,
                "command_failure_kind": "runtime_failure",
                "trace_path_present": !report.trace_path.trim().is_empty(),
                "stdout_path_present": report.stdout_path.as_deref().is_some(),
                "stderr_path_present": report.stderr_path.as_deref().is_some()
            }),
            RuntimeDisposition::Denied { reason } => {
                let browser = parse_agent_browser_observation(command);
                serde_json::json!({
                    "tool": "bash",
                    "command_len": command.len(),
                    "action_class": classify_bash_action(command).as_str(),
                    "browser_action_class": browser.as_ref().map(|value| value.action_class.as_str()),
                    "disposition": "denied",
                    "command_failure_kind": command_failure_kind_from_reason(reason),
                    "browser_observation": browser.as_ref().map(|value| serde_json::json!({
                        "action_class": value.action_class.as_str(),
                        "session_name": value.session_name.clone(),
                        "session_reusable": false,
                        "target_url": value.target_url.clone(),
                        "page_state": serde_json::Value::Null,
                        "interstitial_kind": serde_json::Value::Null,
                    })),
                    "reason_len": reason.len()
                })
            }
        },
        PlannerToolResult::CodexExec {
            request,
            result,
            failure_reason,
        } => serde_json::json!({
            "tool": "codex_exec",
            "command_argv_len": request.command.len(),
            "sandbox": request.sandbox.as_str(),
            "has_cwd": request.cwd.as_ref().map(|value| !value.trim().is_empty()).unwrap_or(false),
            "writable_roots_count": request.writable_roots.len(),
            "timeout_ms": request.timeout_ms,
            "disposition": if failure_reason.is_some() { "failed" } else { "succeeded" },
            "exit_code": result.as_ref().map(|value| value.exit_code),
            "stdout_len": result.as_ref().map(|value| value.stdout.len()).unwrap_or(0),
            "stderr_len": result.as_ref().map(|value| value.stderr.len()).unwrap_or(0),
            "command_failure_kind": failure_reason.as_ref().map(|_| "runtime_failure"),
        }),
        PlannerToolResult::CodexSession {
            request,
            result,
            failure_reason,
        } => serde_json::json!({
            "tool": "codex_session",
            "sandbox": request.sandbox.as_str(),
            "session_id": request.session_id.clone(),
            "has_cwd": request.cwd.as_ref().map(|value| !value.trim().is_empty()).unwrap_or(false),
            "writable_roots_count": request.writable_roots.len(),
            "local_images_count": request.local_images.len(),
            "disposition": if failure_reason.is_some() { "failed" } else { "succeeded" },
            "status": result.as_ref().map(|value| value.status.as_str()),
            "session_name": result.as_ref().map(|value| value.session_name.clone()),
            "summary_len": result.as_ref().map(|value| value.summary.len()).unwrap_or(0),
            "user_visible_len": result
                .as_ref()
                .and_then(|value| value.user_visible.as_ref())
                .map(|value| value.len())
                .unwrap_or(0),
            "command_failure_kind": failure_reason.as_ref().map(|_| "runtime_failure"),
        }),
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

pub(crate) fn summarize_redacted_tool_result(result: &PlannerToolResult) -> serde_json::Value {
    let mut summary = summarize_observed_tool_result(result);
    strip_raw_artifact_fields(&mut summary);
    summary
}

fn strip_raw_artifact_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("raw_artifacts");
            for nested in map.values_mut() {
                strip_raw_artifact_fields(nested);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                strip_raw_artifact_fields(item);
            }
        }
        _ => {}
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
        "instruction": GUIDANCE_INSTRUCTION_PROMPT
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
            | PlannerGuidanceSignal::ContinueNeedCurrentPageInspection
            | PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial
            | PlannerGuidanceSignal::ContinueNeedCommandReformulation
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
