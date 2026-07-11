#![forbid(unsafe_code)]

mod agent_loop;
mod auth_cli;
mod automation;
mod cli;
mod codex;
mod compose;
mod compose_gate;
mod config;
mod host;
mod ingress;
mod json_util;
mod lcm_integration;
mod logging;
mod media;
mod planner_conversation;
mod planner_feedback;
mod planner_products;
mod planner_progress;
mod render_refs;
mod response_style;
mod setup;
mod shell_words;
mod sieve_home;
mod text_match;
mod turn;

use agent_loop::{run_agent_loop, AgentLoopDeps};
use auth_cli::run_auth_command;
use automation::AutomationManager;
use clap::Parser;
use codex::CodexManager;
use config::{
    approval_allowances_path, load_approval_allowances, load_dotenv_if_present, parse_sieve_home,
    AppConfig,
};
use ingress::{
    spawn_stdin_prompt_loop, spawn_telegram_loop, IngressPrompt, PromptSource, RuntimeBridge,
};
use lcm_integration::LcmIntegration;
use logging::FanoutRuntimeEventLog;
use sieve_command_summaries::{command_available, DefaultCommandSummarizer};
use sieve_llm::{
    GuidanceModel, OpenAiGuidanceModel, OpenAiPlannerModel, OpenAiResponseModel,
    OpenAiSummaryModel, ResponseModel, SummaryModel,
};
use sieve_policy::{canonicalize_net_origin_scope, TomlPolicyEngine};
use sieve_quarantine::BwrapQuarantineRunner;
use sieve_runtime::{
    AutomationTool, InProcessApprovalBus, RuntimeDeps, RuntimeOrchestrator,
    SystemClock as RuntimeClock,
};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::{Action, InteractionModality, Resource, UncertainMode, UnknownMode};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use tokio::sync::mpsc as tokio_mpsc;
use turn::{run_turn, AppMainlineRunner, TurnDeps};

const EMBEDDED_BASELINE_POLICY: &str = include_str!("../../../docs/policy/baseline-policy.toml");

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IngressMode {
    spawn_stdin_loop: bool,
    spawn_telegram_loop: bool,
}

fn ingress_mode_for_command(command: &cli::CliCommand) -> IngressMode {
    match command {
        cli::CliCommand::Agent => IngressMode {
            spawn_stdin_loop: true,
            spawn_telegram_loop: true,
        },
        cli::CliCommand::Run { .. }
        | cli::CliCommand::Auth(_)
        | cli::CliCommand::Doctor { .. }
        | cli::CliCommand::Setup { .. } => IngressMode {
            spawn_stdin_loop: false,
            spawn_telegram_loop: false,
        },
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    load_dotenv_if_present().map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let cli_command = cli::Cli::parse().into_command();
    if let cli::CliCommand::Doctor { json } = &cli_command {
        host::HostReport::detect()
            .print(*json)
            .map_err(io::Error::other)?;
        return Ok(());
    }
    let sieve_home_git_enabled = initialize_sieve_home_from_env();
    if let cli::CliCommand::Setup { non_interactive } = &cli_command {
        setup::run_setup_command(*non_interactive).await?;
        return Ok(());
    }
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
    let mut ingress_mode = ingress_mode_for_command(&cli_command);
    let single_command_mode = cli_prompt.is_some();

    let mut cfg = AppConfig::from_env_with_telegram(false)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    ingress_mode.spawn_telegram_loop &= cfg.telegram_enabled();
    let policy_override = env::var("SIEVE_POLICY_PATH")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let policy_toml = load_policy_toml(&cfg.policy_path, policy_override)?;
    let policy = TomlPolicyEngine::from_toml_str(&policy_toml)?;
    cfg.allowed_net_connect_scopes = planner_allowed_net_connect_scopes(&policy);
    if !host::quarantine_available() {
        if cfg.unknown_mode != UnknownMode::Deny || cfg.uncertain_mode != UncertainMode::Deny {
            eprintln!(
                "quarantine unavailable (requires bash, bwrap, and strace); unknown and uncertain commands will be denied"
            );
        }
        cfg.unknown_mode = UnknownMode::Deny;
        cfg.uncertain_mode = UncertainMode::Deny;
    }
    let lcm = if cfg.lcm.enabled {
        Some(Arc::new(
            LcmIntegration::new(cfg.lcm.clone()).map_err(io::Error::other)?,
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
    let event_log = Arc::new(FanoutRuntimeEventLog::new(
        cfg.event_log_path.clone(),
        event_tx.clone(),
    )?);
    let codex: Option<Arc<dyn sieve_runtime::CodexTool>> = if host::codex_available() {
        Some(Arc::new(CodexManager::new(
            cfg.codex_store_path.clone(),
            approval_bus.clone(),
            event_log.clone(),
        )?))
    } else {
        None
    };
    let (prompt_tx, prompt_rx, _stdin_thread, bridge) = if ingress_mode.spawn_stdin_loop {
        let (prompt_tx, prompt_rx) = tokio_mpsc::unbounded_channel();
        let stdin_thread = spawn_stdin_prompt_loop(prompt_tx.clone());
        (
            Some(prompt_tx.clone()),
            Some(prompt_rx),
            Some(stdin_thread),
            Some(RuntimeBridge::with_prompt_tx(
                approval_bus.clone(),
                codex.clone(),
                prompt_tx,
            )),
        )
    } else {
        (None, None, None, None)
    };
    let telegram_thread = if ingress_mode.spawn_telegram_loop {
        Some(spawn_telegram_loop(
            &cfg,
            bridge.expect("telegram ingress bridge missing"),
            event_rx,
        ))
    } else {
        drop(event_rx);
        None
    };
    let typing_tx = event_tx.clone();
    let _sieve_home_watcher = sieve_home_git_enabled
        .then(|| sieve_home::spawn_sieve_home_git_watcher(cfg.sieve_home.clone()));

    let automation = match prompt_tx {
        Some(prompt_tx) => Some(Arc::new(AutomationManager::new(
            &cfg,
            prompt_tx,
            Arc::new(RuntimeClock),
        )?)),
        None => None,
    };
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
        codex,
        approval_bus,
        event_log: event_log.clone(),
        clock: Arc::new(RuntimeClock),
    }));
    if let Some(automation) = automation.clone() {
        tokio::spawn(automation.run_loop());
    }
    let turn_deps = TurnDeps {
        runtime: runtime.clone(),
        guidance_model: guidance_model.clone(),
        response_model: response_model.clone(),
        summary_model: summary_model.clone(),
        lcm: lcm.clone(),
        event_log: event_log.clone(),
        cfg: cfg.clone(),
    };
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
        run_turn(&turn_deps, reserved_turn.run_id, &cli_prompt).await?;
        drop(turn_deps);
        drop(runtime);
        drop(event_log);
        if let Some(telegram_thread) = telegram_thread {
            let _ = telegram_thread.join();
        }
    } else {
        run_agent_loop(
            AgentLoopDeps {
                turn: turn_deps,
                automation: automation.clone(),
                telegram_tx: typing_tx,
            },
            prompt_rx.expect("agent mode prompt receiver missing"),
        )
        .await;
    }

    Ok(())
}

fn load_policy_toml(path: &Path, explicit_override: bool) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(policy) => Ok(policy),
        Err(err) if err.kind() == io::ErrorKind::NotFound && !explicit_override => {
            Ok(EMBEDDED_BASELINE_POLICY.to_string())
        }
        Err(err) => Err(err),
    }
}

fn initialize_sieve_home_from_env() -> bool {
    let sieve_home = parse_sieve_home(env::var("SIEVE_HOME").ok(), env::var("HOME").ok());
    if !command_available("git") {
        if let Err(err) = sieve_home::ensure_sieve_home_dirs(&sieve_home) {
            eprintln!(
                "failed to initialize sieve home at {}: {}",
                sieve_home.display(),
                err
            );
        }
        return false;
    }
    if let Err(err) = sieve_home::ensure_sieve_home_repo(&sieve_home) {
        eprintln!(
            "failed to initialize sieve home repo at {}: {}",
            sieve_home.display(),
            err
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests;
