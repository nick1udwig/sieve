use super::{package_hint, CapabilityReport, CapabilityStatus, HostReport, InstallPlan};
use sieve_command_summaries::{planner_command_catalog, resolve_command};
use sieve_types::PlannerCommandSpec;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub(crate) trait HostProbe {
    fn os(&self) -> &str;
    fn architecture(&self) -> &str;
    fn env(&self, key: &str) -> Option<String>;
    fn resolve_command(&self, command: &str) -> Option<PathBuf>;
    fn current_user_is_root(&self) -> bool;

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }
}

struct SystemHostProbe;

impl HostProbe for SystemHostProbe {
    fn os(&self) -> &str {
        env::consts::OS
    }

    fn architecture(&self) -> &str {
        env::consts::ARCH
    }

    fn env(&self, key: &str) -> Option<String> {
        env::var(key).ok()
    }

    fn resolve_command(&self, command: &str) -> Option<PathBuf> {
        resolve_command(command)
    }

    fn current_user_is_root(&self) -> bool {
        Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeCapabilityManifest {
    report: HostReport,
    package_manager: Option<String>,
    missing_host_packages: Vec<(&'static str, &'static str)>,
    current_user_is_root: bool,
    sudo_available: bool,
    bash_available: bool,
    quarantine_available: bool,
    codex_available: bool,
    git_available: bool,
    planner_commands: Vec<PlannerCommandSpec>,
}

impl RuntimeCapabilityManifest {
    pub(crate) fn current() -> &'static Self {
        static MANIFEST: OnceLock<RuntimeCapabilityManifest> = OnceLock::new();
        MANIFEST.get_or_init(|| Self::detect_with(&SystemHostProbe))
    }

    pub(crate) fn detect_with(probe: &dyn HostProbe) -> Self {
        let package_manager = detect_package_manager(probe);
        let lcm_command = non_empty_probe_env(probe, "SIEVE_LCM_CLI_BIN")
            .unwrap_or_else(|| "sieve-lcm-cli".to_string());
        let bash_available = probe.resolve_command("bash").is_some();
        let git_available = probe.resolve_command("git").is_some();
        let quarantine_available = probe.os() == "linux"
            && ["bash", "bwrap", "strace"]
                .iter()
                .all(|command| probe.resolve_command(command).is_some());
        let codex_available = non_empty_probe_env(probe, "SIEVE_CODEX_APP_SERVER_WS_URL").is_some()
            || non_empty_probe_env(probe, "SIEVE_CODEX_APP_SERVER_BIN").map_or_else(
                || probe.resolve_command("codex").is_some(),
                |program| probe.resolve_command(&program).is_some(),
            );
        let capabilities = detect_capabilities(probe, &package_manager, &lcm_command);
        let core_ready = ["model_config", "model_auth"].iter().all(|id| {
            capabilities
                .iter()
                .find(|capability| capability.id == *id)
                .is_some_and(|capability| capability.status == CapabilityStatus::Available)
        });
        let missing_host_packages = recommended_candidates(probe.os())
            .iter()
            .filter(|(command, _)| probe.resolve_command(command).is_none())
            .copied()
            .collect();
        let planner_commands = planner_command_catalog()
            .iter()
            .filter_map(|descriptor| {
                let command = if descriptor.command == "sieve-lcm-cli" {
                    lcm_command.as_str()
                } else {
                    descriptor.command
                };
                probe.resolve_command(command)?;
                Some(PlannerCommandSpec {
                    command: command.to_string(),
                    description: descriptor.description.replace("sieve-lcm-cli", command),
                })
            })
            .collect();
        Self {
            report: HostReport {
                schema_version: 1,
                os: probe.os().to_string(),
                architecture: probe.architecture().to_string(),
                package_manager: package_manager.clone(),
                core_ready,
                capabilities,
            },
            package_manager,
            missing_host_packages,
            current_user_is_root: probe.current_user_is_root(),
            sudo_available: probe.resolve_command("sudo").is_some(),
            bash_available,
            quarantine_available,
            codex_available,
            git_available,
            planner_commands,
        }
    }

    pub(crate) fn report(&self) -> HostReport {
        debug_assert_eq!(
            self.git_available(),
            self.report.capabilities.iter().any(|capability| {
                capability.id == "git_history" && capability.status == CapabilityStatus::Available
            })
        );
        self.report.clone()
    }

    pub(crate) fn recommended_install_plan(&self) -> Option<InstallPlan> {
        let manager = self.package_manager.as_deref()?;
        if self.missing_host_packages.is_empty() {
            return None;
        }
        let packages = self
            .missing_host_packages
            .iter()
            .map(|(_, package)| *package)
            .collect::<Vec<_>>();
        let features = self
            .missing_host_packages
            .iter()
            .map(|(command, _)| *command)
            .collect::<Vec<_>>();
        super::package_manager_install_plan_with_privilege(
            manager,
            &packages,
            features,
            self.current_user_is_root,
            self.sudo_available,
        )
    }

    pub(crate) fn bash_available(&self) -> bool {
        self.bash_available
    }

    pub(crate) fn quarantine_available(&self) -> bool {
        self.quarantine_available
    }

    pub(crate) fn codex_available(&self) -> bool {
        self.codex_available
    }

    pub(crate) fn git_available(&self) -> bool {
        self.git_available
    }

    pub(crate) fn planner_commands(&self) -> &[PlannerCommandSpec] {
        &self.planner_commands
    }
}

fn detect_capabilities(
    probe: &dyn HostProbe,
    package_manager: &Option<String>,
    lcm_command: &str,
) -> Vec<CapabilityReport> {
    vec![
        executable_capability(
            probe,
            "shell",
            "Known shell-command execution",
            &["bash"],
            package_hint(package_manager, &["bash"]),
        ),
        quarantine_capability(probe, package_manager),
        executable_capability(
            probe,
            "git_history",
            "Sieve-home version history",
            &["git"],
            package_hint(package_manager, &["git"]),
        ),
        executable_capability(
            probe,
            "web_fetch",
            "Planner web fetching",
            &["curl"],
            package_hint(package_manager, &["curl"]),
        ),
        alternative_capability(
            probe,
            "web_search",
            "Brave web-search CLI",
            &["bravesearch", "brave-search"],
            Some("install bravesearch, then rerun sieve doctor".to_string()),
        ),
        executable_capability(
            probe,
            "browser",
            "Browser automation",
            &["agent-browser"],
            Some("install agent-browser, then rerun sieve doctor".to_string()),
        ),
        codex_capability(probe),
        executable_capability(
            probe,
            "audio",
            "Voice transcription and synthesis",
            &["st", "ffmpeg"],
            Some("install st and ffmpeg, then rerun sieve doctor".to_string()),
        ),
        executable_capability(
            probe,
            "trash",
            "Recoverable file deletion",
            &["trash"],
            package_hint(package_manager, &["trash-cli"]),
        ),
        executable_capability(
            probe,
            "memory_cli",
            "Explicit LCM memory queries",
            &[lcm_command],
            Some("install sieve-lcm-cli, then rerun sieve doctor".to_string()),
        ),
        executable_capability(
            probe,
            "google_workspace",
            "Google Workspace operations",
            &["gws"],
            Some("install gws, then rerun sieve doctor".to_string()),
        ),
        executable_capability(
            probe,
            "file_search",
            "Fast workspace search",
            &["rg"],
            package_hint(package_manager, &["ripgrep"]),
        ),
        telegram_capability(probe),
        model_config_capability(probe),
        model_auth_capability(probe),
    ]
}

fn quarantine_capability(
    probe: &dyn HostProbe,
    package_manager: &Option<String>,
) -> CapabilityReport {
    if probe.os() != "linux" {
        return CapabilityReport {
            id: "quarantine",
            description: "Sandboxed unknown-command tracing",
            status: CapabilityStatus::Unsupported,
            detail: "Linux bubblewrap/strace quarantine is unavailable on this OS".to_string(),
            install_hint: None,
        };
    }
    executable_capability(
        probe,
        "quarantine",
        "Sandboxed unknown-command tracing",
        &["bash", "bwrap", "strace"],
        package_hint(package_manager, &["bash", "bubblewrap", "strace"]),
    )
}

fn codex_capability(probe: &dyn HostProbe) -> CapabilityReport {
    if let Some(url) = non_empty_probe_env(probe, "SIEVE_CODEX_APP_SERVER_WS_URL") {
        return CapabilityReport {
            id: "codex",
            description: "Delegated coding and image OCR",
            status: CapabilityStatus::Available,
            detail: format!("configured app-server at {url}"),
            install_hint: None,
        };
    }
    let program = non_empty_probe_env(probe, "SIEVE_CODEX_APP_SERVER_BIN")
        .unwrap_or_else(|| "codex".to_string());
    executable_capability(
        probe,
        "codex",
        "Delegated coding and image OCR",
        &[program.as_str()],
        Some("npm install -g @openai/codex".to_string()),
    )
}

fn telegram_capability(probe: &dyn HostProbe) -> CapabilityReport {
    let configured = non_empty_probe_env(probe, "TELEGRAM_BOT_TOKEN").is_some()
        && non_empty_probe_env(probe, "TELEGRAM_CHAT_ID").is_some();
    configured_capability(
        "telegram",
        "Telegram ingress and approvals",
        configured,
        "credentials configured",
        "optional credentials not configured",
        "run sieve setup to configure Telegram",
    )
}

fn model_config_capability(probe: &dyn HostProbe) -> CapabilityReport {
    let model = non_empty_probe_env(probe, "SIEVE_PLANNER_MODEL");
    CapabilityReport {
        id: "model_config",
        description: "Primary model selection",
        status: if model.is_some() {
            CapabilityStatus::Available
        } else {
            CapabilityStatus::Unconfigured
        },
        detail: model
            .map(|model| format!("planner model {model}"))
            .unwrap_or_else(|| "no planner model selected".to_string()),
        install_hint: Some("run sieve setup to select a model provider and model".to_string()),
    }
}

fn model_auth_capability(probe: &dyn HostProbe) -> CapabilityReport {
    let env_auth = [
        "SIEVE_PLANNER_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_CODEX_ACCESS_TOKEN",
    ]
    .iter()
    .any(|key| non_empty_probe_env(probe, key).is_some());
    let auth_path = sieve_llm::resolve_openai_codex_auth_json_path(&|key| probe.env(key));
    configured_capability(
        "model_auth",
        "Planner and response model authentication",
        env_auth || probe.is_file(&auth_path),
        "model credentials found",
        "no model credentials found",
        "set OPENAI_API_KEY or run sieve auth login openai-codex",
    )
}

fn configured_capability(
    id: &'static str,
    description: &'static str,
    configured: bool,
    available_detail: &'static str,
    missing_detail: &'static str,
    hint: &'static str,
) -> CapabilityReport {
    CapabilityReport {
        id,
        description,
        status: if configured {
            CapabilityStatus::Available
        } else {
            CapabilityStatus::Unconfigured
        },
        detail: if configured {
            available_detail.to_string()
        } else {
            missing_detail.to_string()
        },
        install_hint: Some(hint.to_string()),
    }
}

fn executable_capability(
    probe: &dyn HostProbe,
    id: &'static str,
    description: &'static str,
    commands: &[&str],
    install_hint: Option<String>,
) -> CapabilityReport {
    let resolved = commands
        .iter()
        .map(|command| (*command, probe.resolve_command(command)))
        .collect::<Vec<_>>();
    let missing = resolved
        .iter()
        .filter_map(|(command, path)| path.is_none().then_some(*command))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return CapabilityReport {
            id,
            description,
            status: CapabilityStatus::Available,
            detail: resolved
                .iter()
                .filter_map(|(_, path)| path.as_ref())
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            install_hint: None,
        };
    }
    CapabilityReport {
        id,
        description,
        status: CapabilityStatus::Missing,
        detail: format!("missing {}", missing.join(", ")),
        install_hint,
    }
}

fn alternative_capability(
    probe: &dyn HostProbe,
    id: &'static str,
    description: &'static str,
    commands: &[&str],
    install_hint: Option<String>,
) -> CapabilityReport {
    if let Some(path) = commands
        .iter()
        .find_map(|command| probe.resolve_command(command))
    {
        return CapabilityReport {
            id,
            description,
            status: CapabilityStatus::Available,
            detail: path.display().to_string(),
            install_hint: None,
        };
    }
    CapabilityReport {
        id,
        description,
        status: CapabilityStatus::Missing,
        detail: format!("missing {}", commands.join(" or ")),
        install_hint,
    }
}

fn detect_package_manager(probe: &dyn HostProbe) -> Option<String> {
    ["apt-get", "dnf", "pacman", "brew", "pkg"]
        .iter()
        .find(|command| probe.resolve_command(command).is_some())
        .map(|command| (*command).to_string())
}

fn recommended_candidates(os: &str) -> &'static [(&'static str, &'static str)] {
    match os {
        "linux" => &[
            ("bash", "bash"),
            ("bwrap", "bubblewrap"),
            ("strace", "strace"),
            ("git", "git"),
            ("curl", "curl"),
            ("rg", "ripgrep"),
            ("trash", "trash-cli"),
        ],
        "macos" => &[
            ("git", "git"),
            ("curl", "curl"),
            ("rg", "ripgrep"),
            ("trash", "trash-cli"),
        ],
        _ => &[],
    }
}

fn non_empty_probe_env(probe: &dyn HostProbe, key: &str) -> Option<String> {
    probe
        .env(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
