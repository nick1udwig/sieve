use super::bash::{
    classify_bash_action, command_failure_kind_from_reason, command_targets_likely_asset,
    command_targets_markdown_view, BashActionClass, MIN_PRIMARY_FETCH_STDOUT_BYTES,
};
use super::browser::{
    infer_browser_page_state, infer_interstitial_kind, parse_agent_browser_observation,
};
use crate::json_util::to_json_value;
use crate::turn::{format_integrity, mainline_artifact_kind_name};
use serde::Serialize;
use sieve_runtime::{
    MainlineArtifactKind, MainlineRunReport, PlannerToolResult, RuntimeDisposition,
};
use std::fs;

const MAX_GUIDANCE_ARTIFACT_EXCERPT_BYTES: usize = 1_600;

#[derive(Serialize)]
struct ArtifactExcerptPayload {
    ref_id: String,
    kind: &'static str,
    byte_count: u64,
    line_count: u64,
    excerpt: Option<String>,
}

#[derive(Serialize)]
struct BrowserObservationPayload {
    action_class: &'static str,
    session_name: Option<String>,
    session_reusable: bool,
    target_url: Option<String>,
    page_state: Option<&'static str>,
    interstitial_kind: Option<&'static str>,
}

#[derive(Serialize)]
struct AutomationObservationPayload {
    tool: &'static str,
    action: &'static str,
    target: Option<&'static str>,
    schedule_kind: Option<&'static str>,
    has_schedule: bool,
    has_prompt: bool,
    has_job_id: bool,
    disposition: &'static str,
    message_len: usize,
    failure_reason_len: usize,
    command_failure_kind: Option<&'static str>,
}

#[derive(Serialize)]
struct BashObservationPayload {
    tool: &'static str,
    command_len: usize,
    action_class: &'static str,
    browser_action_class: Option<&'static str>,
    disposition: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_artifacts: Option<Vec<ArtifactExcerptPayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command_failure_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    browser_observation: Option<BrowserObservationPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    likely_has_candidate_urls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    likely_has_primary_content: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uses_markdown_view: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    likely_asset_target: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_path_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout_path_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_path_present: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_len: Option<usize>,
}

#[derive(Serialize)]
struct CodexExecObservationPayload {
    tool: &'static str,
    command_argv_len: usize,
    sandbox: &'static str,
    has_cwd: bool,
    writable_roots_count: usize,
    timeout_ms: Option<u64>,
    disposition: &'static str,
    exit_code: Option<i32>,
    stdout_len: usize,
    stderr_len: usize,
    command_failure_kind: Option<&'static str>,
}

#[derive(Serialize)]
struct CodexSessionObservationPayload {
    tool: &'static str,
    sandbox: &'static str,
    session_id: Option<String>,
    has_cwd: bool,
    writable_roots_count: usize,
    local_images_count: usize,
    disposition: &'static str,
    status: Option<&'static str>,
    session_name: Option<String>,
    summary_len: usize,
    user_visible_len: usize,
    command_failure_kind: Option<&'static str>,
}

#[derive(Serialize)]
struct EndorseObservationPayload {
    tool: &'static str,
    value_ref_len: usize,
    target_integrity: &'static str,
    applied: bool,
}

#[derive(Serialize)]
struct DeclassifyObservationPayload {
    tool: &'static str,
    value_ref_len: usize,
    sink_len: usize,
    applied: bool,
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

fn artifact_excerpt_payloads(report: &MainlineRunReport) -> Vec<ArtifactExcerptPayload> {
    report
        .artifacts
        .iter()
        .filter(|artifact| artifact.byte_count > 0)
        .take(2)
        .map(|artifact| ArtifactExcerptPayload {
            ref_id: artifact.ref_id.clone(),
            kind: mainline_artifact_kind_name(artifact.kind),
            byte_count: artifact.byte_count,
            line_count: artifact.line_count,
            excerpt: read_artifact_excerpt(&artifact.path),
        })
        .collect()
}

pub(super) fn summarize_observed_tool_result(result: &PlannerToolResult) -> serde_json::Value {
    match result {
        PlannerToolResult::Automation {
            request,
            message,
            failure_reason,
            ..
        } => to_json_value(
            AutomationObservationPayload {
                tool: "automation",
                action: request.action.as_str(),
                target: request.target.as_ref().map(|value| value.as_str()),
                schedule_kind: request.schedule.as_ref().map(|value| value.kind_str()),
                has_schedule: request.schedule.is_some(),
                has_prompt: request
                    .prompt
                    .as_ref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false),
                has_job_id: request
                    .job_id
                    .as_ref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false),
                disposition: if failure_reason.is_some() {
                    "failed"
                } else {
                    "succeeded"
                },
                message_len: message.as_ref().map(|value| value.len()).unwrap_or(0),
                failure_reason_len: failure_reason
                    .as_ref()
                    .map(|value| value.len())
                    .unwrap_or(0),
                command_failure_kind: failure_reason.as_ref().map(|_| "invalid_arguments"),
            },
            "automation observation payload",
        ),
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
                    BrowserObservationPayload {
                        action_class: browser.action_class.as_str(),
                        session_name: browser.session_name.clone(),
                        session_reusable: true,
                        target_url: browser.target_url.clone(),
                        page_state: Some(page_state),
                        interstitial_kind,
                    }
                });
                let failure_kind = if report.exit_code.unwrap_or(1) != 0 {
                    Some("runtime_failure")
                } else if stdout_bytes == 0 && stderr_bytes == 0 {
                    Some("empty_output")
                } else {
                    None
                };
                to_json_value(
                    BashObservationPayload {
                        tool: "bash",
                        command_len: command.len(),
                        action_class: action_class.as_str(),
                        browser_action_class: browser
                            .as_ref()
                            .map(|browser| browser.action_class.as_str()),
                        disposition: "execute_mainline",
                        exit_code: report.exit_code,
                        artifact_count: Some(report.artifacts.len()),
                        stdout_bytes: Some(stdout_bytes),
                        stderr_bytes: Some(stderr_bytes),
                        raw_artifacts: Some(artifact_excerpt_payloads(report)),
                        command_failure_kind: failure_kind,
                        browser_observation,
                        likely_has_candidate_urls: Some(
                            matches!(action_class, BashActionClass::Discovery) && stdout_bytes > 0,
                        ),
                        likely_has_primary_content: Some(
                            matches!(action_class, BashActionClass::Fetch)
                                && stdout_bytes >= MIN_PRIMARY_FETCH_STDOUT_BYTES
                                && !command_targets_likely_asset(command),
                        ),
                        uses_markdown_view: Some(command_targets_markdown_view(command)),
                        likely_asset_target: Some(command_targets_likely_asset(command)),
                        trace_path_present: None,
                        stdout_path_present: None,
                        stderr_path_present: None,
                        reason_len: None,
                    },
                    "bash mainline observation payload",
                )
            }
            RuntimeDisposition::ExecuteQuarantine(report) => to_json_value(
                BashObservationPayload {
                    tool: "bash",
                    command_len: command.len(),
                    action_class: classify_bash_action(command).as_str(),
                    browser_action_class: parse_agent_browser_observation(command)
                        .map(|browser| browser.action_class.as_str()),
                    disposition: "execute_quarantine",
                    exit_code: report.exit_code,
                    artifact_count: None,
                    stdout_bytes: None,
                    stderr_bytes: None,
                    raw_artifacts: None,
                    command_failure_kind: Some("runtime_failure"),
                    browser_observation: None,
                    likely_has_candidate_urls: None,
                    likely_has_primary_content: None,
                    uses_markdown_view: None,
                    likely_asset_target: None,
                    trace_path_present: Some(!report.trace_path.trim().is_empty()),
                    stdout_path_present: Some(report.stdout_path.as_deref().is_some()),
                    stderr_path_present: Some(report.stderr_path.as_deref().is_some()),
                    reason_len: None,
                },
                "bash quarantine observation payload",
            ),
            RuntimeDisposition::Denied { reason } => {
                let browser = parse_agent_browser_observation(command);
                to_json_value(
                    BashObservationPayload {
                        tool: "bash",
                        command_len: command.len(),
                        action_class: classify_bash_action(command).as_str(),
                        browser_action_class: browser
                            .as_ref()
                            .map(|value| value.action_class.as_str()),
                        disposition: "denied",
                        exit_code: None,
                        artifact_count: None,
                        stdout_bytes: None,
                        stderr_bytes: None,
                        raw_artifacts: None,
                        command_failure_kind: Some(command_failure_kind_from_reason(reason)),
                        browser_observation: browser.as_ref().map(|value| {
                            BrowserObservationPayload {
                                action_class: value.action_class.as_str(),
                                session_name: value.session_name.clone(),
                                session_reusable: false,
                                target_url: value.target_url.clone(),
                                page_state: None,
                                interstitial_kind: None,
                            }
                        }),
                        likely_has_candidate_urls: None,
                        likely_has_primary_content: None,
                        uses_markdown_view: None,
                        likely_asset_target: None,
                        trace_path_present: None,
                        stdout_path_present: None,
                        stderr_path_present: None,
                        reason_len: Some(reason.len()),
                    },
                    "bash denied observation payload",
                )
            }
        },
        PlannerToolResult::CodexExec {
            request,
            result,
            failure_reason,
        } => to_json_value(
            CodexExecObservationPayload {
                tool: "codex_exec",
                command_argv_len: request.command.len(),
                sandbox: request.sandbox.as_str(),
                has_cwd: request
                    .cwd
                    .as_ref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false),
                writable_roots_count: request.writable_roots.len(),
                timeout_ms: request.timeout_ms,
                disposition: if failure_reason.is_some() {
                    "failed"
                } else {
                    "succeeded"
                },
                exit_code: result.as_ref().map(|value| value.exit_code),
                stdout_len: result.as_ref().map(|value| value.stdout.len()).unwrap_or(0),
                stderr_len: result.as_ref().map(|value| value.stderr.len()).unwrap_or(0),
                command_failure_kind: failure_reason.as_ref().map(|_| "runtime_failure"),
            },
            "codex exec observation payload",
        ),
        PlannerToolResult::CodexSession {
            request,
            result,
            failure_reason,
        } => to_json_value(
            CodexSessionObservationPayload {
                tool: "codex_session",
                sandbox: request.sandbox.as_str(),
                session_id: request.session_id.clone(),
                has_cwd: request
                    .cwd
                    .as_ref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false),
                writable_roots_count: request.writable_roots.len(),
                local_images_count: request.local_images.len(),
                disposition: if failure_reason.is_some() {
                    "failed"
                } else {
                    "succeeded"
                },
                status: result.as_ref().map(|value| value.status.as_str()),
                session_name: result.as_ref().map(|value| value.session_name.clone()),
                summary_len: result
                    .as_ref()
                    .map(|value| value.summary.len())
                    .unwrap_or(0),
                user_visible_len: result
                    .as_ref()
                    .and_then(|value| value.user_visible.as_ref())
                    .map(|value| value.len())
                    .unwrap_or(0),
                command_failure_kind: failure_reason.as_ref().map(|_| "runtime_failure"),
            },
            "codex session observation payload",
        ),
        PlannerToolResult::Endorse {
            request,
            transition,
        } => to_json_value(
            EndorseObservationPayload {
                tool: "endorse",
                value_ref_len: request.value_ref.0.len(),
                target_integrity: format_integrity(request.target_integrity),
                applied: transition.is_some(),
            },
            "endorse observation payload",
        ),
        PlannerToolResult::Declassify {
            request,
            transition,
        } => to_json_value(
            DeclassifyObservationPayload {
                tool: "declassify",
                value_ref_len: request.value_ref.0.len(),
                sink_len: request.sink.0.len(),
                applied: transition.is_some(),
            },
            "declassify observation payload",
        ),
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
