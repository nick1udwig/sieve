#![forbid(unsafe_code)]

mod config;
mod lcm_integration;
mod logging;
mod media;
mod render_refs;
mod response_style;

use async_trait::async_trait;
use config::{
    approval_allowances_path, load_approval_allowances, load_dotenv_from_path,
    load_dotenv_if_present, parse_policy_path, parse_sieve_home,
    parse_telegram_allowed_sender_user_ids, persist_runtime_approval_allowances,
    runtime_event_log_path, save_approval_allowances, AppConfig, DEFAULT_POLICY_PATH,
};
use lcm_integration::{LcmIntegration, LcmIntegrationConfig};
use logging::{
    append_jsonl_record, append_turn_controller_event, now_ms, ConversationLogRecord,
    ConversationRole, FanoutRuntimeEventLog, TelegramLoopEvent,
};
use render_refs::{
    read_artifact_as_string, render_assistant_message, resolve_ref_summary_input, RenderRef,
};
use response_style::{
    compact_single_line, concise_style_diagnostic, dedupe_preserve_order,
    denied_outcomes_only_message, enforce_link_policy, extract_plain_urls_from_text,
    filter_non_asset_urls, obvious_meta_compose_pattern, strip_asset_urls_from_message,
    strip_unexpanded_render_tokens, user_requested_detailed_output, user_requested_sources,
};
use serde::{Deserialize, Serialize};
use sieve_command_summaries::DefaultCommandSummarizer;
use sieve_interface_telegram::{
    SystemClock as TelegramClock, TelegramAdapter, TelegramAdapterConfig, TelegramBotApiLongPoll,
    TelegramEventBridge, TelegramPrompt,
};
use sieve_llm::{
    GuidanceModel, OpenAiGuidanceModel, OpenAiPlannerModel, OpenAiResponseModel,
    OpenAiSummaryModel, ResponseModel, ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput,
    SummaryModel, SummaryRequest,
};
use sieve_policy::TomlPolicyEngine;
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{
    ApprovalBusError, EventLogError, InProcessApprovalBus, MainlineArtifact, MainlineArtifactKind,
    MainlineRunError, MainlineRunReport, MainlineRunRequest, MainlineRunner, PlannerRunRequest,
    PlannerRunResult, PlannerToolResult, RuntimeDeps, RuntimeDisposition, RuntimeEventLog,
    RuntimeOrchestrator, SystemClock as RuntimeClock,
};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{
    Action, ApprovalResolvedEvent, AssistantMessageEvent, Capability, Integrity,
    InteractionModality, ModalityContract, ModalityOverrideReason, PlannerGuidanceFrame,
    PlannerGuidanceInput, PlannerGuidanceSignal, Resource, RunId, RuntimeEvent, UncertainMode,
    UnknownMode,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc as tokio_mpsc, Semaphore};

fn planner_allowed_tools_for_turn(
    configured_tools: &[String],
    has_known_value_refs: bool,
) -> Vec<String> {
    if has_known_value_refs {
        return configured_tools.to_vec();
    }

    configured_tools
        .iter()
        .filter(|tool| tool.as_str() != "endorse" && tool.as_str() != "declassify")
        .cloned()
        .collect()
}

fn planner_allowed_net_connect_scopes(policy: &TomlPolicyEngine) -> Vec<String> {
    let mut scopes = Vec::new();
    let mut seen = BTreeSet::new();
    for capability in &policy.config().allow_capabilities {
        if capability.resource != Resource::Net || capability.action != Action::Connect {
            continue;
        }
        let planner_scope = planner_net_connect_scope(&capability.scope);
        if seen.insert(planner_scope.clone()) {
            scopes.push(planner_scope);
        }
    }
    scopes
}

fn planner_net_connect_scope(scope: &str) -> String {
    let Ok(url) = reqwest::Url::parse(scope) else {
        return scope.to_string();
    };
    let Some(host) = url.host_str() else {
        return scope.to_string();
    };
    let mut origin = format!("{}://{}", url.scheme(), host.to_ascii_lowercase());
    if let Some(port) = url.port() {
        let default_port = match url.scheme() {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        if Some(port) != default_port {
            origin.push(':');
            origin.push_str(&port.to_string());
        }
    }
    origin
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSource {
    Stdin,
    Telegram,
}

impl PromptSource {
    fn as_str(self) -> &'static str {
        match self {
            PromptSource::Stdin => "stdin",
            PromptSource::Telegram => "telegram",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IngressPrompt {
    source: PromptSource,
    text: String,
    modality: InteractionModality,
    media_file_id: Option<String>,
}

struct RuntimeBridge {
    approval_bus: Arc<InProcessApprovalBus>,
    prompt_tx: Option<tokio_mpsc::UnboundedSender<IngressPrompt>>,
}

impl RuntimeBridge {
    fn new(approval_bus: Arc<InProcessApprovalBus>) -> Self {
        Self {
            approval_bus,
            prompt_tx: None,
        }
    }

    fn with_prompt_tx(
        approval_bus: Arc<InProcessApprovalBus>,
        prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
    ) -> Self {
        Self {
            approval_bus,
            prompt_tx: Some(prompt_tx),
        }
    }
}

impl TelegramEventBridge for RuntimeBridge {
    fn publish_runtime_event(&self, _event: &RuntimeEvent) {}

    fn submit_approval(&self, approval: ApprovalResolvedEvent) {
        if let Err(err) = self.approval_bus.resolve(approval) {
            eprintln!(
                "telegram bridge failed to resolve approval: {}",
                format_approval_bus_error(&err)
            );
        }
    }

    fn submit_prompt(&self, prompt: TelegramPrompt) {
        let text = prompt.text.trim().to_string();
        if prompt.modality == InteractionModality::Text && text.is_empty() {
            return;
        }
        if let Some(prompt_tx) = &self.prompt_tx {
            if let Err(err) = prompt_tx.send(IngressPrompt {
                source: PromptSource::Telegram,
                text,
                modality: prompt.modality,
                media_file_id: prompt.media_file_id,
            }) {
                eprintln!("failed to enqueue telegram prompt: {err}");
            }
        }
    }
}

fn format_approval_bus_error(err: &ApprovalBusError) -> String {
    err.to_string()
}

struct AppMainlineRunner {
    artifact_root: PathBuf,
    next_artifact_id: AtomicU64,
}

impl AppMainlineRunner {
    fn new(artifact_root: PathBuf) -> Self {
        Self {
            artifact_root,
            next_artifact_id: AtomicU64::new(1),
        }
    }

    fn next_ref_id(&self) -> String {
        let next = self.next_artifact_id.fetch_add(1, Ordering::Relaxed);
        format!("artifact-{}-{next}", now_ms())
    }

    async fn persist_artifact(
        &self,
        run_id: &RunId,
        kind: MainlineArtifactKind,
        bytes: &[u8],
    ) -> Result<MainlineArtifact, MainlineRunError> {
        let ref_id = self.next_ref_id();
        let kind_name = match kind {
            MainlineArtifactKind::Stdout => "stdout",
            MainlineArtifactKind::Stderr => "stderr",
        };
        let run_dir = self.artifact_root.join(&run_id.0);
        tokio::fs::create_dir_all(&run_dir)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("create artifact dir failed: {err}")))?;
        let path = run_dir.join(format!("{ref_id}-{kind_name}.log"));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| MainlineRunError::Exec(format!("persist artifact failed: {err}")))?;

        Ok(MainlineArtifact {
            ref_id,
            kind,
            path: path.to_string_lossy().to_string(),
            byte_count: bytes.len() as u64,
            line_count: count_newlines(bytes),
        })
    }
}

#[async_trait]
impl MainlineRunner for AppMainlineRunner {
    async fn run(
        &self,
        request: MainlineRunRequest,
    ) -> Result<MainlineRunReport, MainlineRunError> {
        let output = TokioCommand::new("bash")
            .arg("-lc")
            .arg(&request.script)
            .current_dir(&request.cwd)
            .output()
            .await
            .map_err(|err| MainlineRunError::Exec(err.to_string()))?;

        let stdout_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stdout,
                &output.stdout,
            )
            .await?;
        let stderr_artifact = self
            .persist_artifact(
                &request.run_id,
                MainlineArtifactKind::Stderr,
                &output.stderr,
            )
            .await?;

        Ok(MainlineRunReport {
            run_id: request.run_id,
            exit_code: output.status.code(),
            artifacts: vec![stdout_artifact, stderr_artifact],
        })
    }
}

fn count_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|byte| **byte == b'\n').count() as u64
}

fn spawn_telegram_loop(
    cfg: &AppConfig,
    bridge: RuntimeBridge,
    event_rx: Receiver<TelegramLoopEvent>,
) -> thread::JoinHandle<()> {
    let bot_token = cfg.telegram_bot_token.clone();
    let chat_id = cfg.telegram_chat_id;
    let poll_timeout_secs = cfg.telegram_poll_timeout_secs;
    let allowed_sender_user_ids = cfg.telegram_allowed_sender_user_ids.clone();

    thread::spawn(move || {
        let mut adapter = TelegramAdapter::new(
            TelegramAdapterConfig {
                chat_id,
                poll_timeout_secs,
                allowed_sender_user_ids,
            },
            bridge,
            TelegramBotApiLongPoll::new(bot_token),
            TelegramClock,
        );

        loop {
            let mut disconnected = false;
            loop {
                match event_rx.try_recv() {
                    Ok(TelegramLoopEvent::Runtime(event)) => {
                        if let Err(err) = adapter.publish_runtime_event(event) {
                            eprintln!("telegram publish runtime event failed: {err:?}");
                        }
                    }
                    Ok(TelegramLoopEvent::TypingStart { run_id }) => {
                        if let Err(err) = adapter.start_typing(run_id) {
                            eprintln!("telegram typing start failed: {err:?}");
                        }
                    }
                    Ok(TelegramLoopEvent::TypingStop { run_id }) => {
                        adapter.stop_typing(&run_id);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }

            if disconnected {
                break;
            }

            if let Err(err) = adapter.poll_once() {
                eprintln!("telegram poll failed: {err:?}");
                thread::sleep(Duration::from_secs(1));
            }
        }
    })
}

fn spawn_stdin_prompt_loop(
    prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(line) => {
                    let prompt = line.trim();
                    if prompt.is_empty() {
                        continue;
                    }
                    if let Err(err) = prompt_tx.send(IngressPrompt {
                        source: PromptSource::Stdin,
                        text: prompt.to_string(),
                        modality: InteractionModality::Text,
                        media_file_id: None,
                    }) {
                        eprintln!("stdin prompt loop stopped: {err}");
                        break;
                    }
                }
                Err(err) => {
                    eprintln!("stdin read failed: {err}");
                    break;
                }
            }
        }
    })
}

fn build_response_turn_input(
    run_id: &RunId,
    trusted_user_message: &str,
    response_modality: InteractionModality,
    planner_result: &PlannerRunResult,
) -> (ResponseTurnInput, BTreeMap<String, RenderRef>) {
    let mut render_refs = BTreeMap::new();
    let mut tool_outcomes = Vec::with_capacity(planner_result.tool_results.len());
    for tool_result in &planner_result.tool_results {
        tool_outcomes.push(summarize_tool_result(tool_result, &mut render_refs));
    }

    (
        ResponseTurnInput {
            run_id: run_id.clone(),
            trusted_user_message: trusted_user_message.to_string(),
            response_modality,
            planner_thoughts: planner_result.thoughts.clone(),
            tool_outcomes,
        },
        render_refs,
    )
}

fn requires_output_visibility(input: &ResponseTurnInput) -> bool {
    !non_empty_output_ref_ids(input).is_empty()
        && user_explicitly_requests_output_visibility(&input.trusted_user_message)
}

fn user_explicitly_requests_output_visibility(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("output")
        || lower.contains("stdout")
        || lower.contains("stderr")
        || lower.contains("contents of")
        || lower.contains("content of")
        || lower.contains("show the result")
        || lower.contains("show me the result")
        || lower.contains("run exactly")
        || (lower.contains("what did") && lower.contains("return"))
}

fn output_ref_requires_visibility(kind: &str) -> bool {
    matches!(kind, "stdout" | "stderr")
}

fn non_empty_output_ref_ids(input: &ResponseTurnInput) -> BTreeSet<String> {
    input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|ref_metadata| {
            output_ref_requires_visibility(&ref_metadata.kind) && ref_metadata.byte_count > 0
        })
        .map(|ref_metadata| ref_metadata.ref_id.clone())
        .collect()
}

fn response_evidence_fingerprint(input: &ResponseTurnInput) -> String {
    let mut parts = Vec::new();
    for outcome in &input.tool_outcomes {
        parts.push(format!(
            "{}|{}|{}|{}",
            outcome.tool_name,
            outcome.outcome,
            outcome.attempted_command.as_deref().unwrap_or(""),
            outcome.failure_reason.as_deref().unwrap_or("")
        ));
        for metadata in &outcome.refs {
            parts.push(format!(
                "ref:{}:{}:{}:{}",
                metadata.ref_id, metadata.kind, metadata.byte_count, metadata.line_count
            ));
        }
    }
    parts.join("\n")
}

fn response_has_visible_selected_output(
    input: &ResponseTurnInput,
    response: &sieve_llm::ResponseTurnOutput,
) -> bool {
    let output_ref_ids = non_empty_output_ref_ids(input);
    response.referenced_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id) && response.message.contains(&format!("[[ref:{ref_id}]]"))
    }) || response.summarized_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id)
            && response.message.contains(&format!("[[summary:{ref_id}]]"))
    })
}

fn summarize_tool_result(
    result: &PlannerToolResult,
    render_refs: &mut BTreeMap<String, RenderRef>,
) -> ResponseToolOutcome {
    match result {
        PlannerToolResult::Bash {
            disposition,
            command,
        } => match disposition {
            RuntimeDisposition::ExecuteMainline(report) => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: format!("executed mainline (exit_code={:?})", report.exit_code),
                attempted_command: Some(command.clone()),
                failure_reason: None,
                refs: report
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        render_refs.insert(
                            artifact.ref_id.clone(),
                            RenderRef::Artifact {
                                path: PathBuf::from(&artifact.path),
                                byte_count: artifact.byte_count,
                                line_count: artifact.line_count,
                            },
                        );
                        ResponseRefMetadata {
                            ref_id: artifact.ref_id.clone(),
                            kind: mainline_artifact_kind_name(artifact.kind).to_string(),
                            byte_count: artifact.byte_count,
                            line_count: artifact.line_count,
                        }
                    })
                    .collect(),
            },
            RuntimeDisposition::ExecuteQuarantine(report) => {
                let trace_ref = format!("trace:{}", report.run_id.0);
                render_refs.insert(
                    trace_ref.clone(),
                    RenderRef::Literal {
                        value: report.trace_path.clone(),
                    },
                );
                ResponseToolOutcome {
                    tool_name: "bash".to_string(),
                    outcome: format!(
                        "executed in quarantine (exit_code={:?}, trace=[[ref:{}]])",
                        report.exit_code, trace_ref
                    ),
                    attempted_command: Some(command.clone()),
                    failure_reason: None,
                    refs: vec![ResponseRefMetadata {
                        ref_id: trace_ref,
                        kind: "trace_path".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    }],
                }
            }
            RuntimeDisposition::Denied { reason } => ResponseToolOutcome {
                tool_name: "bash".to_string(),
                outcome: "denied".to_string(),
                attempted_command: Some(command.clone()),
                failure_reason: Some(reason.clone()),
                refs: Vec::new(),
            },
        },
        PlannerToolResult::Endorse {
            request,
            transition,
        } => {
            let value_ref_id = format!("value:{}", request.value_ref.0);
            render_refs.insert(
                value_ref_id.clone(),
                RenderRef::Literal {
                    value: request.value_ref.0.clone(),
                },
            );
            let outcome = match transition {
                Some(transition) => format!(
                    "endorse applied for [[ref:{}]] ({} -> {})",
                    value_ref_id,
                    format_integrity(transition.from_integrity),
                    format_integrity(transition.to_integrity),
                ),
                None => format!("endorse not applied for [[ref:{}]]", value_ref_id),
            };
            ResponseToolOutcome {
                tool_name: "endorse".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: None,
                refs: vec![ResponseRefMetadata {
                    ref_id: value_ref_id,
                    kind: "value_ref".to_string(),
                    byte_count: 0,
                    line_count: 0,
                }],
            }
        }
        PlannerToolResult::Declassify {
            request,
            transition,
        } => {
            let value_ref_id = format!("value:{}", request.value_ref.0);
            let sink_ref_id = format!("sink:{}", request.sink.0);
            render_refs.insert(
                value_ref_id.clone(),
                RenderRef::Literal {
                    value: request.value_ref.0.clone(),
                },
            );
            render_refs.insert(
                sink_ref_id.clone(),
                RenderRef::Literal {
                    value: request.sink.0.clone(),
                },
            );
            let outcome = match transition {
                Some(transition) => format!(
                    "declassify applied for [[ref:{}]] -> [[ref:{}]] (already_allowed={})",
                    value_ref_id, sink_ref_id, transition.sink_was_already_allowed
                ),
                None => format!(
                    "declassify not applied for [[ref:{}]] -> [[ref:{}]]",
                    value_ref_id, sink_ref_id
                ),
            };
            ResponseToolOutcome {
                tool_name: "declassify".to_string(),
                outcome,
                attempted_command: None,
                failure_reason: None,
                refs: vec![
                    ResponseRefMetadata {
                        ref_id: value_ref_id,
                        kind: "value_ref".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                    ResponseRefMetadata {
                        ref_id: sink_ref_id,
                        kind: "sink".to_string(),
                        byte_count: 0,
                        line_count: 0,
                    },
                ],
            }
        }
    }
}

fn mainline_artifact_kind_name(kind: MainlineArtifactKind) -> &'static str {
    match kind {
        MainlineArtifactKind::Stdout => "stdout",
        MainlineArtifactKind::Stderr => "stderr",
    }
}

fn format_integrity(integrity: Integrity) -> &'static str {
    match integrity {
        Integrity::Trusted => "trusted",
        Integrity::Untrusted => "untrusted",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BashActionClass {
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

const MIN_PRIMARY_FETCH_STDOUT_BYTES: u64 = 256;

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

fn classify_bash_action(command: &str) -> BashActionClass {
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

fn command_targets_markdown_view(command: &str) -> bool {
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

fn url_is_likely_asset(url: &str) -> bool {
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

fn has_repeated_bash_outcome(tool_results: &[PlannerToolResult]) -> bool {
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

fn build_guidance_prompt(
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

fn guidance_requests_continue(signal: PlannerGuidanceSignal) -> bool {
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

fn guidance_continue_decision(
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

fn progress_contract_override_signal(
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

fn planner_policy_feedback(tool_results: &[PlannerToolResult]) -> Option<String> {
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

fn trim_for_prompt(value: &str, max_chars: usize) -> String {
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

async fn planner_memory_feedback(tool_results: &[PlannerToolResult]) -> Option<String> {
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

async fn summarize_with_ref_id(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    ref_id: &str,
    payload: &serde_json::Value,
) -> Option<String> {
    let content = payload.to_string();
    let request = SummaryRequest {
        run_id: run_id.clone(),
        ref_id: ref_id.to_string(),
        byte_count: content.len() as u64,
        line_count: count_newlines(content.as_bytes()),
        content,
    };
    match summary_model.summarize_ref(request).await {
        Ok(summary) => {
            let trimmed = summary.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        }
        Err(_) => None,
    }
}

async fn summarize_with_ref_id_counted(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    ref_id: &str,
    payload: &serde_json::Value,
    summary_calls: &mut usize,
    budget_remaining: usize,
) -> Option<String> {
    if *summary_calls >= budget_remaining {
        return None;
    }
    *summary_calls = summary_calls.saturating_add(1);
    summarize_with_ref_id(summary_model, run_id, ref_id, payload).await
}

fn extract_trusted_evidence_lines(
    trusted_user_message: &str,
    planner_thoughts: Option<&str>,
) -> Vec<String> {
    let mut lines = vec![format!("[user] {trusted_user_message}")];
    if let Some(thoughts) = planner_thoughts {
        for line in thoughts.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("[user] ") {
                lines.push(trimmed.to_string());
            }
        }
    }
    dedupe_preserve_order(lines)
}

#[cfg(test)]
fn compose_quality_requires_retry(
    composed_message: &str,
    quality_gate: Option<&str>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    match parse_gate_verdict(quality_gate) {
        None | Some(GateVerdict::Pass) => None,
        Some(GateVerdict::Revise(reason)) => Some(reason),
    }
}

#[cfg(test)]
fn gate_requires_retry(gate: Option<&str>) -> Option<String> {
    match parse_gate_verdict(gate) {
        None | Some(GateVerdict::Pass) => None,
        Some(GateVerdict::Revise(reason)) => Some(reason),
    }
}

fn combine_gate_reasons(gates: &[Option<String>]) -> Option<String> {
    let mut combined = Vec::new();
    for gate in gates {
        if let Some(gate) = gate.as_deref() {
            let trimmed = gate.trim();
            if !trimmed.is_empty() {
                combined.push(trimmed.to_string());
            }
        }
    }
    if combined.is_empty() {
        None
    } else {
        Some(combined.join(" | "))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposePlannerDecision {
    Finalize,
    Continue(PlannerGuidanceSignal),
}

struct ComposeAssistantOutcome {
    message: String,
    quality_gate: Option<String>,
    planner_decision: ComposePlannerDecision,
    summary_calls: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GateVerdict {
    Pass,
    Revise(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ComposeGateOutput {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    continue_code: Option<u16>,
}

fn parse_gate_verdict(gate: Option<&str>) -> Option<GateVerdict> {
    let gate = gate.unwrap_or("").trim();
    if gate.is_empty() {
        return None;
    }
    let lower = gate.to_ascii_lowercase();
    if let Some(revise_idx) = lower.find("revise") {
        let reason = gate[revise_idx + "revise".len()..]
            .trim_start_matches(|ch: char| ch == ':' || ch == '-' || ch.is_whitespace())
            .trim();
        if reason.is_empty() {
            return Some(GateVerdict::Revise("requested revision".to_string()));
        }
        return Some(GateVerdict::Revise(reason.to_string()));
    }
    if lower.starts_with("pass") || (lower.contains("pass") && !lower.contains("revise")) {
        return Some(GateVerdict::Pass);
    }
    Some(GateVerdict::Revise(format!(
        "unstructured gate output: {}",
        compact_single_line(gate, 200)
    )))
}

fn followup_signal_from_reason(
    reason: &str,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let has_tool_context = response_input
        .tool_outcomes
        .iter()
        .any(|outcome| !outcome.refs.is_empty() || outcome.failure_reason.is_some());
    if !has_tool_context {
        return None;
    }

    let lower = reason.to_ascii_lowercase();
    let is_style_only = lower.contains("third-person")
        || lower.contains("third person")
        || lower.contains("meta narration")
        || lower.contains("tone");
    if is_style_only {
        return None;
    }

    let denied_command_present = response_input.tool_outcomes.iter().any(|outcome| {
        outcome
            .failure_reason
            .as_deref()
            .map(|reason| {
                let reason = reason.to_ascii_lowercase();
                reason.contains("denied")
                    || reason.contains("blocked")
                    || reason.contains("not allowed")
                    || reason.contains("unknown command")
            })
            .unwrap_or(false)
    });
    if denied_command_present
        || lower.contains("denied")
        || lower.contains("blocked")
        || lower.contains("not allowed")
        || lower.contains("unknown command")
        || lower.contains("tool failure")
    {
        return Some(PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool);
    }

    if lower.contains("conflict")
        || lower.contains("contradict")
        || lower.contains("inconsistent")
        || lower.contains("disagree")
    {
        return Some(PlannerGuidanceSignal::ContinueResolveSourceConflict);
    }

    if lower.contains("stale")
        || lower.contains("outdated")
        || lower.contains("latest")
        || lower.contains("fresh")
        || lower.contains("current as of")
        || lower.contains("time-bound")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence);
    }

    if lower.contains("no progress")
        || lower.contains("repeated")
        || lower.contains("same command")
        || lower.contains("no evidence gain")
    {
        return Some(PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction);
    }

    if lower.contains("asset")
        || lower.contains("favicon")
        || lower.contains("image url")
        || lower.contains("non-content url")
        || lower.contains("canonical content page")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl);
    }

    if lower.contains("extract url")
        || lower.contains("url extraction")
        || lower.contains("parse urls")
        || lower.contains("extract links")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedUrlExtraction);
    }

    if lower.contains("primary source")
        || lower.contains("primary-page")
        || lower.contains("primary content")
        || lower.contains("discovery/search snippets")
        || lower.contains("snippet-only")
        || lower.contains("insufficient source")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch);
    }

    if lower.contains("higher quality")
        || lower.contains("low quality source")
        || lower.contains("needs citation")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedHigherQualitySource);
    }

    if lower.contains("missing parameter")
        || lower.contains("need user input")
        || lower.contains("needs clarification")
        || lower.contains("please specify")
        || lower.contains("missing required")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedRequiredParameter);
    }

    if lower.contains("preference")
        || lower.contains("constraint")
        || lower.contains("format")
        || lower.contains("units")
        || lower.contains("locale")
    {
        return Some(PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint);
    }

    // Fallback stays generic to avoid domain-specific keyword routing.
    Some(PlannerGuidanceSignal::ContinueRefineApproach)
}

#[cfg(test)]
fn compose_quality_followup_signal(
    quality_gate: Option<&str>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let reason = match parse_gate_verdict(quality_gate) {
        None | Some(GateVerdict::Pass) => return None,
        Some(GateVerdict::Revise(reason)) => reason,
    };
    followup_signal_from_reason(&reason, response_input)
}

fn continue_signal_from_code(code: u16) -> Option<PlannerGuidanceSignal> {
    PlannerGuidanceSignal::try_from(code)
        .ok()
        .filter(|signal| guidance_requests_continue(*signal))
}

fn parse_compose_gate_output(raw: Option<&str>) -> Option<ComposeGateOutput> {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(parsed) = serde_json::from_str::<ComposeGateOutput>(raw) {
        let verdict = parsed.verdict.trim().to_ascii_uppercase();
        let reason = parsed.reason.map(|value| value.trim().to_string());
        if verdict == "PASS" {
            return Some(ComposeGateOutput {
                verdict,
                reason: None,
                continue_code: parsed
                    .continue_code
                    .and_then(continue_signal_from_code)
                    .map(|s| s.code()),
            });
        }
        return Some(ComposeGateOutput {
            verdict: "REVISE".to_string(),
            reason: reason
                .filter(|value| !value.is_empty())
                .or_else(|| Some("requested revision".to_string())),
            continue_code: parsed
                .continue_code
                .and_then(continue_signal_from_code)
                .map(|signal| signal.code()),
        });
    }
    match parse_gate_verdict(Some(raw)) {
        None => None,
        Some(GateVerdict::Pass) => Some(ComposeGateOutput {
            verdict: "PASS".to_string(),
            reason: None,
            continue_code: None,
        }),
        Some(GateVerdict::Revise(reason)) => Some(ComposeGateOutput {
            verdict: "REVISE".to_string(),
            reason: Some(reason),
            continue_code: None,
        }),
    }
}

fn compose_gate_requires_retry(
    composed_message: &str,
    trusted_user_message: &str,
    gate: Option<&ComposeGateOutput>,
) -> Option<String> {
    if obvious_meta_compose_pattern(composed_message) {
        return Some(
            "response used third-person meta narration; respond directly to user".to_string(),
        );
    }
    if let Some(diagnostic) = concise_style_diagnostic(composed_message, trusted_user_message) {
        return Some(diagnostic);
    }
    let gate = gate?;
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    gate.reason
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some("requested revision".to_string()))
}

fn compose_gate_followup_signal(
    gate: Option<&ComposeGateOutput>,
    response_input: &ResponseTurnInput,
) -> Option<PlannerGuidanceSignal> {
    let gate = gate?;
    if let Some(signal) = gate.continue_code.and_then(continue_signal_from_code) {
        return Some(signal);
    }
    if gate.verdict.eq_ignore_ascii_case("PASS") {
        return None;
    }
    let reason = gate.reason.as_deref().unwrap_or("requested revision");
    followup_signal_from_reason(reason, response_input)
}

async fn write_compose_audit_artifacts(
    sieve_home: &Path,
    run_id: &RunId,
    attempts: &[serde_json::Value],
    final_message: &str,
    output_ref_ids: &[String],
    source_urls: &[String],
    quality_gate: Option<&str>,
    grounding_gate: Option<&str>,
    planner_followup_signal: Option<PlannerGuidanceSignal>,
) -> Result<(), String> {
    let run_dir = sieve_home.join("artifacts").join(&run_id.0);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|err| format!("failed to create compose artifact dir: {err}"))?;

    let mut input_refs = Vec::new();
    for (idx, attempt) in attempts.iter().enumerate() {
        let ref_id = format!("assistant-compose-input:{}:{}", run_id.0, idx + 1);
        let path = run_dir.join(format!("assistant-compose-input-{}.json", idx + 1));
        let content = serde_json::to_vec_pretty(attempt)
            .map_err(|err| format!("failed to encode compose payload: {err}"))?;
        tokio::fs::write(&path, content)
            .await
            .map_err(|err| format!("failed to write compose payload artifact: {err}"))?;
        input_refs.push(serde_json::json!({
            "ref_id": ref_id,
            "path": path.to_string_lossy(),
        }));
    }

    let output_ref_id = format!("assistant-compose-output:{}", run_id.0);
    let output_path = run_dir.join("assistant-compose-output.txt");
    tokio::fs::write(&output_path, final_message.as_bytes())
        .await
        .map_err(|err| format!("failed to write compose output artifact: {err}"))?;

    let logs_path = sieve_home.join("logs/compose-events.jsonl");
    let record = serde_json::json!({
        "schema_version": 1,
        "event": "compose_audit",
        "created_at_ms": now_ms(),
        "run_id": run_id.0,
        "input_refs": input_refs,
        "output_ref": {
            "ref_id": output_ref_id,
            "path": output_path.to_string_lossy(),
        },
        "output_ref_ids": output_ref_ids,
        "source_urls": source_urls,
        "quality_gate": quality_gate,
        "grounding_gate": grounding_gate,
        "planner_followup_signal_code": planner_followup_signal.map(PlannerGuidanceSignal::code),
    });
    append_jsonl_record(&logs_path, &record).await
}

async fn collect_source_urls_from_refs(
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
) -> Vec<String> {
    let mut urls = Vec::new();
    let mut seen = BTreeSet::new();
    for outcome in &response_input.tool_outcomes {
        for metadata in &outcome.refs {
            if metadata.byte_count == 0 {
                continue;
            }
            let Some((content, _, _)) =
                resolve_ref_summary_input(&metadata.ref_id, render_refs).await
            else {
                continue;
            };
            for url in extract_plain_urls_from_text(&content) {
                if seen.insert(url.clone()) {
                    urls.push(url);
                }
                if urls.len() >= 8 {
                    return urls;
                }
            }
        }
    }
    urls
}

async fn build_compose_evidence_summaries(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Vec<String> {
    let mut summaries = Vec::new();
    let mut seen = BTreeSet::new();
    for (idx, metadata) in response_input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|metadata| metadata.byte_count > 0)
        .enumerate()
    {
        if idx >= 4 {
            break;
        }
        if !seen.insert(metadata.ref_id.clone()) {
            continue;
        }
        let Some((content, _, _)) = resolve_ref_summary_input(&metadata.ref_id, render_refs).await
        else {
            continue;
        };
        let cache_key = format!(
            "{}:{}:{}:{}",
            trusted_user_message, metadata.ref_id, metadata.byte_count, metadata.line_count
        );
        if let Some(summary) = evidence_cache.get(&cache_key) {
            if !summary.trim().is_empty() {
                summaries.push(summary.clone());
            }
            continue;
        }
        let payload = serde_json::json!({
            "task": "compose_evidence_extract",
            "trusted_user_message": trusted_user_message,
            "ref_id": metadata.ref_id,
            "content": content,
        });
        let ref_id = format!("assistant-compose-evidence:{}:{}", run_id.0, idx + 1);
        if let Some(summary) = summarize_with_ref_id_counted(
            summary_model,
            run_id,
            &ref_id,
            &payload,
            summary_calls,
            summary_budget,
        )
        .await
        {
            let trimmed = summary.trim();
            if !trimmed.is_empty() {
                summaries.push(trimmed.to_string());
                evidence_cache.insert(cache_key, trimmed.to_string());
            }
        }
    }
    summaries
}

async fn run_compose_gate(
    summary_model: &dyn SummaryModel,
    run_id: &RunId,
    trusted_user_message: &str,
    trusted_evidence: &[String],
    composed_message: &str,
    evidence_summaries: &[String],
    source_urls: &[String],
    summary_calls: &mut usize,
    summary_budget: usize,
) -> Option<ComposeGateOutput> {
    let payload = serde_json::json!({
        "task": "compose_gate",
        "trusted_user_message": trusted_user_message,
        "user_requested_sources": user_requested_sources(trusted_user_message),
        "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
        "trusted_evidence": trusted_evidence,
        "composed_message": composed_message,
        "evidence_summaries": evidence_summaries,
        "source_urls": source_urls,
    });
    let raw = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-compose-gate:{}", run_id.0),
        &payload,
        summary_calls,
        summary_budget,
    )
    .await;
    parse_compose_gate_output(raw.as_deref())
}

async fn compose_assistant_message(
    summary_model: &dyn SummaryModel,
    sieve_home: &Path,
    run_id: &RunId,
    trusted_user_message: &str,
    response_input: &ResponseTurnInput,
    render_refs: &BTreeMap<String, RenderRef>,
    draft_message: String,
    evidence_cache: &mut BTreeMap<String, String>,
    summary_budget: usize,
) -> ComposeAssistantOutcome {
    let mut summary_calls = 0usize;
    let output_ref_ids: Vec<String> = non_empty_output_ref_ids(response_input)
        .into_iter()
        .collect();
    let mut source_urls = dedupe_preserve_order(extract_plain_urls_from_text(&draft_message));
    source_urls.extend(collect_source_urls_from_refs(response_input, render_refs).await);
    source_urls = filter_non_asset_urls(dedupe_preserve_order(source_urls));
    let trusted_evidence = extract_trusted_evidence_lines(
        trusted_user_message,
        response_input.planner_thoughts.as_deref(),
    );
    let evidence_summaries = build_compose_evidence_summaries(
        summary_model,
        run_id,
        trusted_user_message,
        response_input,
        render_refs,
        evidence_cache,
        &mut summary_calls,
        summary_budget,
    )
    .await;
    let tool_outcomes: Vec<serde_json::Value> = response_input
        .tool_outcomes
        .iter()
        .map(|outcome| {
            serde_json::json!({
                "tool_name": outcome.tool_name,
                "outcome": outcome.outcome,
                "attempted_command": outcome.attempted_command,
                "failure_reason": outcome.failure_reason,
                "refs": outcome.refs.iter().map(|ref_metadata| {
                    serde_json::json!({
                        "ref_id": ref_metadata.ref_id,
                        "kind": ref_metadata.kind,
                        "byte_count": ref_metadata.byte_count,
                        "line_count": ref_metadata.line_count,
                    })
                }).collect::<Vec<_>>()
            })
        })
        .collect();

    let mut attempt_payloads = Vec::new();
    let payload = serde_json::json!({
        "task": "compose_user_reply",
        "trusted_user_message": trusted_user_message,
        "response_modality": response_input.response_modality,
        "user_requested_sources": user_requested_sources(trusted_user_message),
        "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
        "trusted_evidence": trusted_evidence.clone(),
        "assistant_draft_message": draft_message,
        "planner_thoughts": response_input.planner_thoughts.clone(),
        "tool_outcomes": tool_outcomes,
        "output_ref_ids": output_ref_ids.clone(),
        "available_plain_urls": source_urls.clone(),
        "evidence_summaries": evidence_summaries.clone(),
    });
    attempt_payloads.push(payload.clone());

    let first_composed = summarize_with_ref_id_counted(
        summary_model,
        run_id,
        &format!("assistant-compose:{}", run_id.0),
        &payload,
        &mut summary_calls,
        summary_budget,
    )
    .await
    .unwrap_or_else(|| {
        payload
            .get("assistant_draft_message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });

    let mut composed = first_composed;
    let mut gate = run_compose_gate(
        summary_model,
        run_id,
        trusted_user_message,
        &trusted_evidence,
        &composed,
        &evidence_summaries,
        &source_urls,
        &mut summary_calls,
        summary_budget,
    )
    .await;
    let mut retry_diagnostics = Vec::new();
    if let Some(diagnostic) =
        compose_gate_requires_retry(&composed, trusted_user_message, gate.as_ref())
    {
        retry_diagnostics.push(diagnostic);
    }
    let did_retry = !retry_diagnostics.is_empty() && summary_calls < summary_budget;
    if did_retry {
        let retry_diagnostic = retry_diagnostics.join(" | ");
        let retry_payload = serde_json::json!({
            "task": "compose_user_reply",
            "trusted_user_message": trusted_user_message,
            "response_modality": response_input.response_modality,
            "user_requested_sources": user_requested_sources(trusted_user_message),
            "user_requested_detailed_output": user_requested_detailed_output(trusted_user_message),
            "trusted_evidence": trusted_evidence.clone(),
            "assistant_draft_message": payload
                .get("assistant_draft_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            "planner_thoughts": response_input.planner_thoughts.clone(),
            "tool_outcomes": payload
                .get("tool_outcomes")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            "output_ref_ids": output_ref_ids.clone(),
            "available_plain_urls": source_urls.clone(),
            "evidence_summaries": evidence_summaries.clone(),
            "compose_diagnostic": retry_diagnostic,
            "previous_composed_message": composed,
        });
        attempt_payloads.push(retry_payload.clone());
        composed = summarize_with_ref_id_counted(
            summary_model,
            run_id,
            &format!("assistant-compose-retry:{}", run_id.0),
            &retry_payload,
            &mut summary_calls,
            summary_budget,
        )
        .await
        .unwrap_or_else(|| {
            retry_payload
                .get("previous_composed_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        gate = run_compose_gate(
            summary_model,
            run_id,
            trusted_user_message,
            &trusted_evidence,
            &composed,
            &evidence_summaries,
            &source_urls,
            &mut summary_calls,
            summary_budget,
        )
        .await;
    }

    let quality_gate = match gate.as_ref() {
        Some(value) if value.verdict.eq_ignore_ascii_case("PASS") => Some("PASS".to_string()),
        Some(value) => Some(format!(
            "REVISE: {}",
            value
                .reason
                .as_deref()
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or("requested revision")
        )),
        None if summary_calls >= summary_budget => {
            Some("REVISE: summary call budget exhausted".to_string())
        }
        None => Some("REVISE: missing gate verdict".to_string()),
    };
    let grounding_gate: Option<String> = None;
    let combined_gate = combine_gate_reasons(&[quality_gate.clone()]);
    let planner_followup_signal = if summary_calls >= summary_budget {
        None
    } else {
        compose_gate_followup_signal(gate.as_ref(), response_input)
    };
    let planner_decision = planner_followup_signal
        .map(ComposePlannerDecision::Continue)
        .unwrap_or(ComposePlannerDecision::Finalize);

    let mut composed = enforce_link_policy(composed, &source_urls, trusted_user_message);
    composed = strip_asset_urls_from_message(&composed);
    if let Some(message) = denied_outcomes_only_message(response_input) {
        composed = message;
    }
    if obvious_meta_compose_pattern(&composed) {
        if let Some(message) = denied_outcomes_only_message(response_input) {
            composed = message;
        } else {
            let draft_fallback = payload
                .get("assistant_draft_message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if !draft_fallback.is_empty() && !obvious_meta_compose_pattern(&draft_fallback) {
                composed = draft_fallback;
            }
        }
    }
    composed = strip_asset_urls_from_message(&composed);
    composed = strip_unexpanded_render_tokens(&composed);
    if let Err(err) = write_compose_audit_artifacts(
        sieve_home,
        run_id,
        &attempt_payloads,
        &composed,
        &output_ref_ids,
        &source_urls,
        quality_gate.as_deref(),
        grounding_gate.as_deref(),
        planner_followup_signal,
    )
    .await
    {
        eprintln!("compose audit write failed for {}: {}", run_id.0, err);
    }
    ComposeAssistantOutcome {
        message: composed,
        quality_gate: combined_gate,
        planner_decision,
        summary_calls,
    }
}

fn default_modality_contract(input: InteractionModality) -> ModalityContract {
    ModalityContract {
        input,
        response: input,
        override_reason: None,
    }
}

fn override_modality_contract(
    contract: &mut ModalityContract,
    response: InteractionModality,
    reason: ModalityOverrideReason,
) {
    contract.response = response;
    contract.override_reason = Some(reason);
}

async fn emit_assistant_error_message(
    event_log: &FanoutRuntimeEventLog,
    run_id: &RunId,
    error_message: String,
) -> Result<(), EventLogError> {
    event_log
        .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
            schema_version: 1,
            run_id: run_id.clone(),
            message: error_message.clone(),
            created_at_ms: now_ms(),
        }))
        .await?;
    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            error_message,
            now_ms(),
        ))
        .await
}

async fn run_turn(
    runtime: &RuntimeOrchestrator,
    guidance_model: &dyn GuidanceModel,
    response_model: &dyn ResponseModel,
    summary_model: &dyn SummaryModel,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: &FanoutRuntimeEventLog,
    cfg: &AppConfig,
    run_index: u64,
    source: PromptSource,
    input_modality: InteractionModality,
    media_file_id: Option<String>,
    user_message: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let run_id = RunId(format!("run-{run_index}"));
    let mut modality_contract = default_modality_contract(input_modality);
    if modality_contract.response == InteractionModality::Image {
        override_modality_contract(
            &mut modality_contract,
            InteractionModality::Text,
            ModalityOverrideReason::NotSupported,
        );
    }
    let (trusted_user_message, input_error) = match input_modality {
        InteractionModality::Text => (user_message.clone(), None),
        InteractionModality::Audio => match media_file_id.as_deref() {
            Some(file_id) => match media::transcribe_audio_prompt(cfg, &run_id, file_id).await {
                Ok(transcript) => (transcript, None),
                Err(err) => (
                    String::new(),
                    Some(format!("audio input unavailable: {err}")),
                ),
            },
            None => (
                String::new(),
                Some("audio input missing media file id".to_string()),
            ),
        },
        InteractionModality::Image => match media_file_id.as_deref() {
            Some(file_id) => {
                match media::extract_image_prompt(
                    &cfg.telegram_bot_token,
                    &cfg.sieve_home,
                    &run_id,
                    file_id,
                )
                .await
                {
                    Ok(extracted) => (extracted, None),
                    Err(err) => (
                        String::new(),
                        Some(format!("image input unavailable: {err}")),
                    ),
                }
            }
            None => (
                String::new(),
                Some("image input missing media file id".to_string()),
            ),
        },
    };
    if let Some(error_message) = input_error {
        println!("{}: {}", run_id.0, error_message);
        emit_assistant_error_message(event_log, &run_id, error_message).await?;
        return Ok(());
    }

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_user_message(&trusted_user_message).await {
            eprintln!("lcm ingest user failed for {}: {}", run_id.0, err);
        }
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::User,
            trusted_user_message.clone(),
            now_ms(),
        ))
        .await?;

    let mut aggregated_result = PlannerRunResult {
        thoughts: None,
        tool_results: Vec::new(),
    };
    let mut planner_guidance: Option<PlannerGuidanceFrame> = None;
    let mut consecutive_empty_steps = 0usize;
    let mut planner_steps_taken = 0usize;
    let mut compose_followup_cycles = 0usize;
    let mut summary_calls_used = 0usize;
    let mut compose_continue_fingerprints = BTreeSet::new();
    let mut compose_evidence_cache = BTreeMap::new();
    let max_compose_followup_cycles = cfg.max_planner_steps.max(1);
    let planner_step_hard_limit = cfg
        .max_planner_steps
        .saturating_add(max_compose_followup_cycles);
    let mut planner_step_limit = cfg.max_planner_steps.max(1);
    let planner_user_message = trusted_user_message.clone();

    let assistant_message = loop {
        while planner_steps_taken < planner_step_limit {
            let step_number = planner_steps_taken + 1;
            let policy_feedback = planner_policy_feedback(&aggregated_result.tool_results);
            let memory_feedback = planner_memory_feedback(&aggregated_result.tool_results).await;
            let planner_turn_user_message = match (policy_feedback, memory_feedback) {
                (Some(policy), Some(memory)) => {
                    format!("{planner_user_message}\n\n{policy}\n\n{memory}")
                }
                (Some(policy), None) => format!("{planner_user_message}\n\n{policy}"),
                (None, Some(memory)) => format!("{planner_user_message}\n\n{memory}"),
                (None, None) => planner_user_message.clone(),
            };
            let has_known_value_refs = runtime.has_known_value_refs()?;
            let allowed_tools_for_turn =
                planner_allowed_tools_for_turn(&cfg.allowed_tools, has_known_value_refs);
            let step_result = match runtime
                .orchestrate_planner_turn(PlannerRunRequest {
                    run_id: run_id.clone(),
                    cwd: cfg.runtime_cwd.clone(),
                    user_message: planner_turn_user_message,
                    allowed_tools: allowed_tools_for_turn,
                    allowed_net_connect_scopes: cfg.allowed_net_connect_scopes.clone(),
                    previous_events: event_log.snapshot(),
                    guidance: planner_guidance.clone(),
                    control_value_refs: BTreeSet::new(),
                    control_endorsed_by: None,
                    unknown_mode: cfg.unknown_mode,
                    uncertain_mode: cfg.uncertain_mode,
                })
                .await
            {
                Ok(result) => result,
                Err(err) => {
                    if let Err(log_err) =
                        emit_assistant_error_message(event_log, &run_id, format!("error: {err}"))
                            .await
                    {
                        eprintln!("failed to append assistant error conversation log: {log_err}");
                    }
                    return Err(err.into());
                }
            };

            planner_steps_taken = planner_steps_taken.saturating_add(1);
            let step_tool_count = step_result.tool_results.len();
            if step_tool_count == 0 {
                consecutive_empty_steps = consecutive_empty_steps.saturating_add(1);
            } else {
                consecutive_empty_steps = 0;
            }
            if let Some(thoughts) = step_result.thoughts.clone() {
                aggregated_result.thoughts = Some(thoughts);
            }
            let step_results = step_result.tool_results;
            aggregated_result.tool_results.extend(step_results.clone());
            if let Err(err) = persist_runtime_approval_allowances(runtime, &cfg.sieve_home) {
                eprintln!(
                    "failed to persist approval allowances for {}: {}",
                    run_id.0, err
                );
            }

            if has_repeated_bash_outcome(&aggregated_result.tool_results) {
                let can_retry =
                    planner_steps_taken < planner_step_limit && consecutive_empty_steps < 2;
                append_turn_controller_event(
                        &cfg.sieve_home,
                        &run_id,
                        "planner_repeat_guard",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                            "reason": "detected repeated bash command/result; forcing action-change guidance",
                            "continue": can_retry,
                            "next_signal_code": PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                        }),
                    )
                    .await;
                if can_retry {
                    planner_guidance = Some(PlannerGuidanceFrame {
                        code: PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction.code(),
                        confidence_bps: 9_000,
                        source_hit_index: None,
                        evidence_ref_index: None,
                    });
                    continue;
                }
                break;
            }

            let guidance_prompt = build_guidance_prompt(
                &trusted_user_message,
                step_number,
                cfg.max_planner_steps,
                &step_results,
                &aggregated_result.tool_results,
            );
            let guidance_output = match guidance_model
                .classify_guidance(PlannerGuidanceInput {
                    run_id: run_id.clone(),
                    prompt: guidance_prompt,
                })
                .await
            {
                Ok(output) => output,
                Err(err) => {
                    eprintln!(
                        "guidance model failed for {} at step {}: {}",
                        run_id.0, step_number, err
                    );
                    append_turn_controller_event(
                        &cfg.sieve_home,
                        &run_id,
                        "planner_guidance_error",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                        }),
                    )
                    .await;
                    break;
                }
            };
            let signal = match guidance_output.guidance.signal() {
                Ok(signal) => signal,
                Err(err) => {
                    eprintln!(
                        "invalid guidance signal for {} at step {}: {}",
                        run_id.0, step_number, err
                    );
                    append_turn_controller_event(
                        &cfg.sieve_home,
                        &run_id,
                        "planner_guidance_invalid",
                        serde_json::json!({
                            "step_number": step_number,
                            "planner_steps_taken": planner_steps_taken,
                        }),
                    )
                    .await;
                    break;
                }
            };
            let override_applied = progress_contract_override_signal(
                &trusted_user_message,
                signal,
                &aggregated_result.tool_results,
            );
            let effective_signal = override_applied
                .map(|(override_signal, _)| override_signal)
                .unwrap_or(signal);
            let (should_continue, next_step_limit, auto_extended_limit) =
                guidance_continue_decision(
                    effective_signal,
                    consecutive_empty_steps,
                    planner_steps_taken,
                    planner_step_limit,
                    planner_step_hard_limit,
                );
            planner_step_limit = next_step_limit;
            append_turn_controller_event(
                &cfg.sieve_home,
                &run_id,
                "planner_guidance",
                serde_json::json!({
                    "step_number": step_number,
                    "signal_code": signal.code(),
                    "effective_signal_code": effective_signal.code(),
                    "override_reason": override_applied.map(|(_, reason)| reason),
                    "continue": should_continue,
                    "step_tool_count": step_tool_count,
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "auto_extended_limit": auto_extended_limit,
                    "consecutive_empty_steps": consecutive_empty_steps,
                }),
            )
            .await;
            let mut guidance_frame = guidance_output.guidance;
            guidance_frame.code = effective_signal.code();
            planner_guidance = Some(guidance_frame);
            if !should_continue {
                break;
            }
        }

        let (response_input, render_refs) = build_response_turn_input(
            &run_id,
            &trusted_user_message,
            modality_contract.response,
            &aggregated_result,
        );
        let mut response_input = response_input;
        let mut response_output = match response_model
            .write_turn_response(response_input.clone())
            .await
        {
            Ok(response) => response,
            Err(err) => {
                if let Err(log_err) =
                    emit_assistant_error_message(event_log, &run_id, format!("error: {err}")).await
                {
                    eprintln!("failed to append assistant error conversation log: {log_err}");
                }
                return Err(err.into());
            }
        };

        if requires_output_visibility(&response_input)
            && !response_has_visible_selected_output(&response_input, &response_output)
        {
            // One regeneration pass: enforce that non-empty output refs are either shown raw
            // or summarized by Q-LLM, without exposing untrusted strings to the model.
            let diagnostics = "Non-empty output refs exist (stdout/stderr). Include at least one output token directly in `message` using [[ref:<id>]] or [[summary:<id>]], and list the same id in referenced_ref_ids or summarized_ref_ids.";
            response_input.planner_thoughts = Some(match response_input.planner_thoughts.take() {
                Some(existing) if !existing.trim().is_empty() => {
                    format!("{existing}\n{diagnostics}")
                }
                _ => diagnostics.to_string(),
            });

            response_output = match response_model
                .write_turn_response(response_input.clone())
                .await
            {
                Ok(response) => response,
                Err(err) => {
                    if let Err(log_err) =
                        emit_assistant_error_message(event_log, &run_id, format!("error: {err}"))
                            .await
                    {
                        eprintln!("failed to append assistant error conversation log: {log_err}");
                    }
                    return Err(err.into());
                }
            };

            if !response_has_visible_selected_output(&response_input, &response_output) {
                if let Some(fallback_ref_id) =
                    non_empty_output_ref_ids(&response_input).into_iter().next()
                {
                    response_output
                        .summarized_ref_ids
                        .insert(fallback_ref_id.clone());
                    let token = format!("[[summary:{fallback_ref_id}]]");
                    if !response_output.message.contains(&token) {
                        let base = response_output.message.trim();
                        response_output.message = if base.is_empty() {
                            token
                        } else {
                            format!("{base}\n{token}")
                        };
                    }
                }
            }
        }

        let output_visibility_required = requires_output_visibility(&response_input);
        let evidence_fingerprint = response_evidence_fingerprint(&response_input);
        let draft_message = if output_visibility_required {
            render_assistant_message(
                &response_output.message,
                &response_output.referenced_ref_ids,
                &response_output.summarized_ref_ids,
                &render_refs,
                summary_model,
                &run_id,
            )
            .await
        } else {
            let stripped = strip_unexpanded_render_tokens(&response_output.message);
            if stripped.trim().is_empty() {
                render_assistant_message(
                    &response_output.message,
                    &response_output.referenced_ref_ids,
                    &response_output.summarized_ref_ids,
                    &render_refs,
                    summary_model,
                    &run_id,
                )
                .await
            } else {
                stripped
            }
        };
        let remaining_summary_budget = cfg
            .max_summary_calls_per_turn
            .saturating_sub(summary_calls_used);
        let composed = if remaining_summary_budget == 0 {
            ComposeAssistantOutcome {
                message: draft_message,
                quality_gate: Some("REVISE: summary call budget exhausted".to_string()),
                planner_decision: ComposePlannerDecision::Finalize,
                summary_calls: 0,
            }
        } else {
            compose_assistant_message(
                summary_model,
                &cfg.sieve_home,
                &run_id,
                &trusted_user_message,
                &response_input,
                &render_refs,
                draft_message,
                &mut compose_evidence_cache,
                remaining_summary_budget,
            )
            .await
        };
        summary_calls_used = summary_calls_used.saturating_add(composed.summary_calls);

        if let ComposePlannerDecision::Continue(signal) = composed.planner_decision {
            let mut can_continue = planner_steps_taken < planner_step_hard_limit
                && compose_followup_cycles < max_compose_followup_cycles;
            let mut continue_block_reason: Option<&str> = None;
            if can_continue && summary_calls_used >= cfg.max_summary_calls_per_turn {
                can_continue = false;
                continue_block_reason = Some("summary_budget_exhausted");
            }
            if can_continue && !compose_continue_fingerprints.insert(evidence_fingerprint.clone()) {
                can_continue = false;
                continue_block_reason = Some("no_new_evidence");
            }
            append_turn_controller_event(
                &cfg.sieve_home,
                &run_id,
                "compose_decision",
                serde_json::json!({
                    "planner_decision_code": signal.code(),
                    "quality_gate_len": composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                    "planner_steps_taken": planner_steps_taken,
                    "planner_step_limit": planner_step_limit,
                    "planner_step_hard_limit": planner_step_hard_limit,
                    "compose_followup_cycles": compose_followup_cycles,
                    "continue": can_continue,
                    "continue_block_reason": continue_block_reason,
                    "summary_calls_used": summary_calls_used,
                    "summary_call_budget": cfg.max_summary_calls_per_turn,
                }),
            )
            .await;
            if can_continue {
                compose_followup_cycles = compose_followup_cycles.saturating_add(1);
                planner_step_limit = planner_step_limit
                    .saturating_add(1)
                    .min(planner_step_hard_limit.max(1));
                planner_guidance = Some(PlannerGuidanceFrame {
                    code: signal.code(),
                    confidence_bps: 9_000,
                    source_hit_index: None,
                    evidence_ref_index: None,
                });
                continue;
            }
        }

        append_turn_controller_event(
            &cfg.sieve_home,
            &run_id,
            "turn_finalize",
            serde_json::json!({
                "planner_steps_taken": planner_steps_taken,
                "planner_step_limit": planner_step_limit,
                "planner_step_hard_limit": planner_step_hard_limit,
                "compose_followup_cycles": compose_followup_cycles,
                "quality_gate_len": composed.quality_gate.as_deref().map(str::len).unwrap_or(0),
                "summary_calls_used": summary_calls_used,
                "summary_call_budget": cfg.max_summary_calls_per_turn,
            }),
        )
        .await;
        break composed.message;
    };
    println!("{}: {}", run_id.0, assistant_message);

    let mut delivered_audio = false;
    if source == PromptSource::Telegram && modality_contract.response == InteractionModality::Audio
    {
        match media::synthesize_audio_reply(cfg, &run_id, &assistant_message).await {
            Ok(audio_path) => {
                if let Err(err) = media::send_telegram_voice(
                    &cfg.telegram_bot_token,
                    cfg.telegram_chat_id,
                    &audio_path,
                )
                .await
                {
                    eprintln!("audio reply delivery failed for {}: {}", run_id.0, err);
                    override_modality_contract(
                        &mut modality_contract,
                        InteractionModality::Text,
                        ModalityOverrideReason::ToolFailure,
                    );
                } else {
                    delivered_audio = true;
                }
            }
            Err(err) => {
                eprintln!("audio synthesis failed for {}: {}", run_id.0, err);
                override_modality_contract(
                    &mut modality_contract,
                    InteractionModality::Text,
                    ModalityOverrideReason::ToolFailure,
                );
            }
        }
    }

    if !delivered_audio {
        event_log
            .append(RuntimeEvent::AssistantMessage(AssistantMessageEvent {
                schema_version: 1,
                run_id: run_id.clone(),
                message: assistant_message.clone(),
                created_at_ms: now_ms(),
            }))
            .await?;
    }

    event_log
        .append_conversation(ConversationLogRecord::new(
            run_id.clone(),
            ConversationRole::Assistant,
            assistant_message.clone(),
            now_ms(),
        ))
        .await?;

    if let Some(memory) = lcm.as_ref() {
        if let Err(err) = memory.ingest_assistant_message(&assistant_message).await {
            eprintln!("lcm ingest assistant failed for {}: {}", run_id.0, err);
        }
    }
    Ok(())
}

struct TypingGuard {
    telegram_tx: Sender<TelegramLoopEvent>,
    run_id: String,
}

impl TypingGuard {
    fn start(
        telegram_tx: Sender<TelegramLoopEvent>,
        run_id: String,
    ) -> Result<Self, mpsc::SendError<TelegramLoopEvent>> {
        telegram_tx.send(TelegramLoopEvent::TypingStart {
            run_id: run_id.clone(),
        })?;
        Ok(Self {
            telegram_tx,
            run_id,
        })
    }
}

impl Drop for TypingGuard {
    fn drop(&mut self) {
        let _ = self.telegram_tx.send(TelegramLoopEvent::TypingStop {
            run_id: self.run_id.clone(),
        });
    }
}

async fn run_agent_loop(
    runtime: Arc<RuntimeOrchestrator>,
    guidance_model: Arc<dyn GuidanceModel>,
    response_model: Arc<dyn ResponseModel>,
    summary_model: Arc<dyn SummaryModel>,
    lcm: Option<Arc<LcmIntegration>>,
    event_log: Arc<FanoutRuntimeEventLog>,
    cfg: AppConfig,
    telegram_tx: Sender<TelegramLoopEvent>,
    mut prompt_rx: tokio_mpsc::UnboundedReceiver<IngressPrompt>,
) {
    let semaphore = Arc::new(Semaphore::new(cfg.max_concurrent_turns));
    let next_run_id = Arc::new(AtomicU64::new(1));

    eprintln!(
        "sieve-app agent mode ready; prompts accepted from stdin + Telegram chat {}",
        cfg.telegram_chat_id
    );

    while let Some(prompt) = prompt_rx.recv().await {
        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };

        let runtime = runtime.clone();
        let guidance_model = guidance_model.clone();
        let response_model = response_model.clone();
        let summary_model = summary_model.clone();
        let lcm = lcm.clone();
        let event_log = event_log.clone();
        let cfg = cfg.clone();
        let telegram_tx = telegram_tx.clone();
        let source = prompt.source;
        let text = prompt.text;
        let modality = prompt.modality;
        let media_file_id = prompt.media_file_id;
        let run_index = next_run_id.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _permit = permit;
            let typing_guard = if source == PromptSource::Telegram {
                TypingGuard::start(telegram_tx, format!("run-{run_index}"))
                    .map(Some)
                    .unwrap_or(None)
            } else {
                None
            };
            if let Err(err) = run_turn(
                &runtime,
                guidance_model.as_ref(),
                response_model.as_ref(),
                summary_model.as_ref(),
                lcm.clone(),
                &event_log,
                &cfg,
                run_index,
                source,
                modality,
                media_file_id,
                text,
            )
            .await
            {
                eprintln!("run-{run_index} ({}) failed: {err}", source.as_str());
            }
            drop(typing_guard);
        });
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv_if_present().map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let cli_prompt = env::args().skip(1).collect::<Vec<String>>().join(" ");
    let single_command_mode = !cli_prompt.trim().is_empty();

    let mut cfg =
        AppConfig::from_env().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    let policy_toml = fs::read_to_string(&cfg.policy_path)?;
    let policy = TomlPolicyEngine::from_toml_str(&policy_toml)?;
    cfg.allowed_net_connect_scopes = planner_allowed_net_connect_scopes(&policy);
    let lcm = if cfg.lcm.enabled {
        Some(Arc::new(
            LcmIntegration::new(cfg.lcm.clone())
                .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?,
        ))
    } else {
        None
    };

    let planner = OpenAiPlannerModel::from_env()?;
    let guidance_model: Arc<dyn GuidanceModel> = Arc::new(OpenAiGuidanceModel::from_env()?);
    let response_model: Arc<dyn ResponseModel> = Arc::new(OpenAiResponseModel::from_env()?);
    let summary_model: Arc<dyn SummaryModel> = Arc::new(OpenAiSummaryModel::from_env()?);
    let approval_bus = Arc::new(InProcessApprovalBus::new());
    let (event_tx, event_rx) = mpsc::channel();
    let (prompt_rx, _stdin_thread, bridge) = if single_command_mode {
        (None, None, RuntimeBridge::new(approval_bus.clone()))
    } else {
        let (prompt_tx, prompt_rx) = tokio_mpsc::unbounded_channel();
        let stdin_thread = spawn_stdin_prompt_loop(prompt_tx.clone());
        (
            Some(prompt_rx),
            Some(stdin_thread),
            RuntimeBridge::with_prompt_tx(approval_bus.clone(), prompt_tx),
        )
    };
    let telegram_thread = spawn_telegram_loop(&cfg, bridge, event_rx);
    let typing_tx = event_tx.clone();
    let event_log = Arc::new(FanoutRuntimeEventLog::new(
        cfg.event_log_path.clone(),
        event_tx,
    )?);

    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
        quarantine: Arc::new(BwrapQuarantineRunner::default()),
        mainline: Arc::new(AppMainlineRunner::new(cfg.sieve_home.join("artifacts"))),
        planner: Arc::new(planner),
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    }));
    let allowances_path = approval_allowances_path(&cfg.sieve_home);
    match load_approval_allowances(&allowances_path) {
        Ok(allowances) => {
            if let Err(err) = runtime.restore_persistent_approval_allowances(&allowances) {
                eprintln!(
                    "failed to restore approval allowances from {}: {}",
                    allowances_path.display(),
                    err
                );
            }
        }
        Err(err) => {
            eprintln!(
                "failed to load approval allowances from {}: {}",
                allowances_path.display(),
                err
            );
        }
    }

    if single_command_mode {
        run_turn(
            &runtime,
            guidance_model.as_ref(),
            response_model.as_ref(),
            summary_model.as_ref(),
            lcm.clone(),
            &event_log,
            &cfg,
            1,
            PromptSource::Stdin,
            InteractionModality::Text,
            None,
            cli_prompt,
        )
        .await?;
        drop(runtime);
        drop(event_log);
        let _ = telegram_thread.join();
    } else {
        run_agent_loop(
            runtime.clone(),
            guidance_model.clone(),
            response_model.clone(),
            summary_model.clone(),
            lcm.clone(),
            event_log.clone(),
            cfg.clone(),
            typing_tx,
            prompt_rx.expect("agent mode prompt receiver missing"),
        )
        .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests;
