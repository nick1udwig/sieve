#![forbid(unsafe_code)]

mod agent_loop;
mod auth_cli;
mod automation;
mod cli;
mod codex;
mod compose;
mod compose_gate;
mod config;
mod ingress;
mod lcm_integration;
mod logging;
mod media;
mod planner_conversation;
mod planner_feedback;
mod planner_products;
mod planner_progress;
mod render_refs;
mod response_style;
mod sieve_home;
mod turn;

use agent_loop::run_agent_loop;
#[cfg(test)]
use async_trait::async_trait;
use auth_cli::run_auth_command;
use automation::AutomationManager;
use clap::Parser;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use cli::{parse_cli_args, AuthCommand, AuthProvider, CliCommand};
use codex::CodexManager;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use compose_gate::{
    combine_gate_reasons, compose_gate_followup_signal, compose_gate_requires_retry,
    compose_quality_followup_signal, compose_quality_requires_retry,
    extract_trusted_evidence_lines, gate_requires_retry, parse_compose_gate_output,
    ComposeGateOutput,
};
use config::{
    approval_allowances_path, load_approval_allowances, load_dotenv_if_present, AppConfig,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use config::{
    load_dotenv_from_path, parse_policy_path, parse_sieve_home,
    parse_telegram_allowed_sender_user_ids, runtime_event_log_path, save_approval_allowances,
    DEFAULT_POLICY_PATH,
};
use ingress::{
    spawn_stdin_prompt_loop, spawn_telegram_loop, IngressPrompt, PromptSource, RuntimeBridge,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use ingress::{TurnKind, TypingGuard};
use lcm_integration::LcmIntegration;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use lcm_integration::LcmIntegrationConfig;
use logging::FanoutRuntimeEventLog;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use logging::{now_ms, ConversationLogRecord, ConversationRole, TelegramLoopEvent};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use planner_conversation::{build_planner_conversation, planner_step_trace_messages};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use planner_feedback::{planner_memory_feedback, planner_policy_feedback};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use planner_products::{PlannerIntermediateProductSummary, PlannerOpaqueHandleStore};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use planner_progress::{
    classify_bash_action, command_targets_markdown_view, guidance_continue_decision,
    guidance_requests_continue, has_repeated_bash_outcome, progress_contract_override_signal,
    url_is_likely_asset, BashActionClass, MIN_PRIMARY_FETCH_STDOUT_BYTES,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use render_refs::{
    read_artifact_as_string, render_assistant_message, resolve_ref_summary_input, RenderRef,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use response_style::{
    compact_single_line, concise_style_diagnostic, dedupe_preserve_order,
    denied_outcomes_only_message, enforce_link_policy, extract_plain_urls_from_text,
    filter_non_asset_urls, obvious_meta_compose_pattern, strip_asset_urls_from_message,
    strip_unexpanded_render_tokens, user_requested_detailed_output, user_requested_sources,
};
use sieve_command_summaries::DefaultCommandSummarizer;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_home::{ensure_sieve_home_repo, maybe_commit_sieve_home_changes};
use sieve_llm::{
    GuidanceModel, OpenAiGuidanceModel, OpenAiPlannerModel, OpenAiResponseModel,
    OpenAiSummaryModel, ResponseModel, SummaryModel,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_llm::{
    ResponseRefMetadata, ResponseToolOutcome, ResponseTurnInput, SummaryRequest,
};
use sieve_policy::{canonicalize_net_origin_scope, TomlPolicyEngine};
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{
    AutomationTool, InProcessApprovalBus, RuntimeDeps, RuntimeOrchestrator,
    SystemClock as RuntimeClock,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_runtime::{
    MainlineArtifact, MainlineArtifactKind, MainlineRunReport, PlannerRunResult, PlannerToolResult,
    RuntimeDisposition, RuntimeEventLog,
};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{Action, InteractionModality, Resource};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_types::{
    ApprovalResolvedEvent, Capability, ModalityOverrideReason, PlannerGuidanceSignal, RunId,
    RuntimeEvent, UncertainMode, UnknownMode,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use std::path::PathBuf;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use std::sync::mpsc::Sender;
use std::sync::Arc;
use tokio::sync::mpsc as tokio_mpsc;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use turn::summarize_with_ref_id_counted;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use turn::{
    build_response_turn_input, default_modality_contract, override_modality_contract,
    planner_allowed_tools_for_turn, requires_output_visibility,
    response_has_visible_selected_output, TurnOutcome,
};
use turn::{run_turn, AppMainlineRunner};
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
    canonicalize_net_origin_scope(scope).unwrap_or_else(|| scope.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv_if_present().map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let cli_command = cli::Cli::parse().into_command();
    if let cli::CliCommand::Auth(command) = cli_command.clone() {
        run_auth_command(command)
            .await
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        return Ok(());
    }

    let cli_prompt = match &cli_command {
        cli::CliCommand::Run { prompt } => Some(prompt.clone()),
        _ => None,
    };
    let single_command_mode = cli_prompt.is_some();

    let mut cfg =
        AppConfig::from_env().map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    if let Err(err) = sieve_home::ensure_sieve_home_repo(&cfg.sieve_home) {
        eprintln!(
            "failed to initialize sieve home repo at {}: {}",
            cfg.sieve_home.display(),
            err
        );
    }
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
    let (prompt_tx, prompt_rx, _stdin_thread, bridge) = if single_command_mode {
        (None, None, None, RuntimeBridge::new(approval_bus.clone()))
    } else {
        let (prompt_tx, prompt_rx) = tokio_mpsc::unbounded_channel();
        let stdin_thread = spawn_stdin_prompt_loop(prompt_tx.clone());
        (
            Some(prompt_tx.clone()),
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
    let _sieve_home_watcher = sieve_home::spawn_sieve_home_git_watcher(cfg.sieve_home.clone());

    let automation = match prompt_tx {
        Some(prompt_tx) => Some(Arc::new(AutomationManager::new(
            &cfg,
            prompt_tx,
            Arc::new(RuntimeClock),
        )?)),
        None => None,
    };
    let codex = Arc::new(CodexManager::new(
        cfg.codex_store_path.clone(),
        approval_bus.clone(),
        event_log.clone(),
    )?);
    let runtime = Arc::new(RuntimeOrchestrator::new(RuntimeDeps {
        shell: Arc::new(BasicShellAnalyzer),
        summaries: Arc::new(DefaultCommandSummarizer),
        policy: Arc::new(policy),
        quarantine: Arc::new(BwrapQuarantineRunner::default()),
        mainline: Arc::new(AppMainlineRunner::new(cfg.sieve_home.join("artifacts"))),
        planner: Arc::new(planner),
        automation: automation
            .clone()
            .map(|manager| -> Arc<dyn AutomationTool> { manager }),
        codex: Some(codex),
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    }));
    if let Some(automation) = automation.clone() {
        tokio::spawn(automation.run_loop());
    }
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
        let cli_prompt = IngressPrompt::user(
            PromptSource::Stdin,
            cli_prompt.expect("single-command prompt missing"),
            InteractionModality::Text,
            None,
        );
        let reserved_turn = event_log.reserve_turn_with_metadata(
            PromptSource::Stdin.as_str(),
            &cli_prompt.session_key,
            cli_prompt.turn_kind.as_str(),
        );
        run_turn(
            &runtime,
            guidance_model.as_ref(),
            response_model.as_ref(),
            summary_model.clone(),
            lcm.clone(),
            &event_log,
            &cfg,
            reserved_turn.run_id,
            &cli_prompt,
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
            automation.clone(),
            typing_tx,
            prompt_rx.expect("agent mode prompt receiver missing"),
        )
        .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests;
