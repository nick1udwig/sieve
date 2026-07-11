use serde::Serialize;
use sieve_command_summaries::resolve_command;
use std::env;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct HostReport {
    schema_version: u16,
    os: String,
    architecture: String,
    package_manager: Option<String>,
    core_ready: bool,
    capabilities: Vec<CapabilityReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CapabilityReport {
    id: &'static str,
    description: &'static str,
    status: CapabilityStatus,
    detail: String,
    install_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CapabilityStatus {
    Available,
    Unconfigured,
    Missing,
    Unsupported,
}

impl HostReport {
    pub(crate) fn detect() -> Self {
        let package_manager = detect_package_manager();
        let lcm_cli = lcm_cli_program();
        let capabilities = vec![
            executable_capability(
                "shell",
                "Known shell-command execution",
                &["bash"],
                package_hint(&package_manager, &["bash"]),
            ),
            quarantine_capability(&package_manager),
            executable_capability(
                "git_history",
                "Sieve-home version history",
                &["git"],
                package_hint(&package_manager, &["git"]),
            ),
            executable_capability(
                "web_fetch",
                "Planner web fetching",
                &["curl"],
                package_hint(&package_manager, &["curl"]),
            ),
            alternative_capability(
                "web_search",
                "Brave web-search CLI",
                &["bravesearch", "brave-search"],
                Some("install bravesearch, then rerun sieve doctor".to_string()),
            ),
            executable_capability(
                "browser",
                "Browser automation",
                &["agent-browser"],
                Some("install agent-browser, then rerun sieve doctor".to_string()),
            ),
            codex_capability(),
            executable_capability(
                "audio",
                "Voice transcription and synthesis",
                &["st", "ffmpeg"],
                Some("install st and ffmpeg, then rerun sieve doctor".to_string()),
            ),
            executable_capability(
                "trash",
                "Recoverable file deletion",
                &["trash"],
                package_hint(&package_manager, &["trash-cli"]),
            ),
            executable_capability(
                "memory_cli",
                "Explicit LCM memory queries",
                &[lcm_cli.as_str()],
                Some("install sieve-lcm-cli, then rerun sieve doctor".to_string()),
            ),
            executable_capability(
                "google_workspace",
                "Google Workspace operations",
                &["gws"],
                Some("install gws, then rerun sieve doctor".to_string()),
            ),
            executable_capability(
                "file_search",
                "Fast workspace search",
                &["rg"],
                package_hint(&package_manager, &["ripgrep"]),
            ),
            telegram_capability(),
            model_config_capability(),
            model_auth_capability(),
        ];
        let core_ready = ["model_config", "model_auth"].iter().all(|id| {
            capabilities
                .iter()
                .find(|capability| capability.id == *id)
                .is_some_and(|capability| capability.status == CapabilityStatus::Available)
        });
        Self {
            schema_version: 1,
            os: env::consts::OS.to_string(),
            architecture: env::consts::ARCH.to_string(),
            package_manager,
            core_ready,
            capabilities,
        }
    }

    pub(crate) fn print(&self, json: bool) -> Result<(), String> {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(self)
                    .map_err(|err| format!("encode doctor report failed: {err}"))?
            );
            return Ok(());
        }
        println!("Sieve host: {} {}", self.os, self.architecture);
        println!(
            "Core: {}",
            if self.core_ready {
                "ready"
            } else {
                "model authentication required"
            }
        );
        for capability in &self.capabilities {
            let marker = match capability.status {
                CapabilityStatus::Available => "ok",
                CapabilityStatus::Unconfigured => "configure",
                CapabilityStatus::Missing => "missing",
                CapabilityStatus::Unsupported => "unsupported",
            };
            println!("[{marker}] {}: {}", capability.id, capability.detail);
            if let Some(hint) = &capability.install_hint {
                if capability.status != CapabilityStatus::Available {
                    println!("  {hint}");
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallPlan {
    pub(crate) missing_features: Vec<&'static str>,
    program: String,
    args: Vec<String>,
}

impl InstallPlan {
    pub(crate) fn display_command(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(crate) fn execute(&self) -> Result<(), String> {
        let status = Command::new(&self.program)
            .args(&self.args)
            .status()
            .map_err(|err| format!("failed to run {}: {err}", self.program))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "dependency install exited with status {}",
                status
                    .code()
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string())
            ))
        }
    }
}

pub(crate) fn recommended_install_plan() -> Option<InstallPlan> {
    let manager = detect_package_manager()?;
    let candidates: &[(&str, &str)] = match env::consts::OS {
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
        _ => return None,
    };
    let missing = candidates
        .iter()
        .filter(|(command, _)| resolve_command(command).is_none())
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return None;
    }
    let packages = missing
        .iter()
        .map(|(_, package)| *package)
        .collect::<Vec<_>>();
    let features = missing
        .iter()
        .map(|(command, _)| *command)
        .collect::<Vec<_>>();
    package_manager_install_plan(&manager, &packages, features)
}

pub(crate) fn quarantine_available() -> bool {
    ["bash", "bwrap", "strace"]
        .iter()
        .all(|command| resolve_command(command).is_some())
}

pub(crate) fn codex_available() -> bool {
    if non_empty_env("SIEVE_CODEX_APP_SERVER_WS_URL").is_some() {
        return true;
    }
    non_empty_env("SIEVE_CODEX_APP_SERVER_BIN").map_or_else(
        || resolve_command("codex").is_some(),
        |program| resolve_command(&program).is_some(),
    )
}

fn quarantine_capability(package_manager: &Option<String>) -> CapabilityReport {
    if env::consts::OS != "linux" {
        return CapabilityReport {
            id: "quarantine",
            description: "Sandboxed unknown-command tracing",
            status: CapabilityStatus::Unsupported,
            detail: "Linux bubblewrap/strace quarantine is unavailable on this OS".to_string(),
            install_hint: None,
        };
    }
    executable_capability(
        "quarantine",
        "Sandboxed unknown-command tracing",
        &["bash", "bwrap", "strace"],
        package_hint(package_manager, &["bash", "bubblewrap", "strace"]),
    )
}

fn codex_capability() -> CapabilityReport {
    if let Some(url) = non_empty_env("SIEVE_CODEX_APP_SERVER_WS_URL") {
        return CapabilityReport {
            id: "codex",
            description: "Delegated coding and image OCR",
            status: CapabilityStatus::Available,
            detail: format!("configured app-server at {url}"),
            install_hint: None,
        };
    }
    let program =
        non_empty_env("SIEVE_CODEX_APP_SERVER_BIN").unwrap_or_else(|| "codex".to_string());
    executable_capability(
        "codex",
        "Delegated coding and image OCR",
        &[program.as_str()],
        Some("npm install -g @openai/codex".to_string()),
    )
}

fn telegram_capability() -> CapabilityReport {
    let configured = non_empty_env("TELEGRAM_BOT_TOKEN").is_some()
        && non_empty_env("TELEGRAM_CHAT_ID").is_some();
    CapabilityReport {
        id: "telegram",
        description: "Telegram ingress and approvals",
        status: if configured {
            CapabilityStatus::Available
        } else {
            CapabilityStatus::Unconfigured
        },
        detail: if configured {
            "credentials configured".to_string()
        } else {
            "optional credentials not configured".to_string()
        },
        install_hint: Some("run sieve setup to configure Telegram".to_string()),
    }
}

fn model_auth_capability() -> CapabilityReport {
    let api_key = [
        "SIEVE_PLANNER_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "OPENAI_CODEX_ACCESS_TOKEN",
    ]
    .iter()
    .any(|key| non_empty_env(key).is_some());
    let auth_file = sieve_llm::resolve_openai_codex_auth_json_path_from_env();
    let configured = api_key || auth_file.is_file();
    CapabilityReport {
        id: "model_auth",
        description: "Planner and response model authentication",
        status: if configured {
            CapabilityStatus::Available
        } else {
            CapabilityStatus::Unconfigured
        },
        detail: if configured {
            "model credentials found".to_string()
        } else {
            "no model credentials found".to_string()
        },
        install_hint: Some("set OPENAI_API_KEY or run sieve auth login openai-codex".to_string()),
    }
}

fn model_config_capability() -> CapabilityReport {
    let model = non_empty_env("SIEVE_PLANNER_MODEL");
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

fn executable_capability(
    id: &'static str,
    description: &'static str,
    commands: &[&str],
    install_hint: Option<String>,
) -> CapabilityReport {
    let resolved = commands
        .iter()
        .map(|command| (*command, resolve_command(command)))
        .collect::<Vec<_>>();
    let missing = resolved
        .iter()
        .filter_map(|(command, path)| path.is_none().then_some(*command))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        let detail = resolved
            .iter()
            .filter_map(|(_, path)| path.as_ref())
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return CapabilityReport {
            id,
            description,
            status: CapabilityStatus::Available,
            detail,
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
    id: &'static str,
    description: &'static str,
    commands: &[&str],
    install_hint: Option<String>,
) -> CapabilityReport {
    if let Some(path) = commands.iter().find_map(|command| resolve_command(command)) {
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

fn detect_package_manager() -> Option<String> {
    ["apt-get", "dnf", "pacman", "brew", "pkg"]
        .iter()
        .find(|command| resolve_command(command).is_some())
        .map(|command| (*command).to_string())
}

fn package_hint(package_manager: &Option<String>, packages: &[&str]) -> Option<String> {
    let joined = packages.join(" ");
    package_manager.as_deref().map(|manager| match manager {
        "apt-get" => format!("sudo apt-get install -y {joined}"),
        "dnf" => format!("sudo dnf install -y {joined}"),
        "pacman" => format!("sudo pacman -S --needed {joined}"),
        "brew" => format!("brew install {joined}"),
        "pkg" => format!("pkg install {joined}"),
        _ => format!("install {joined} with your package manager"),
    })
}

fn package_manager_install_plan(
    manager: &str,
    packages: &[&str],
    missing_features: Vec<&'static str>,
) -> Option<InstallPlan> {
    let (program, mut args) = match manager {
        "apt-get" => (
            "apt-get".to_string(),
            vec!["install".to_string(), "-y".to_string()],
        ),
        "dnf" => (
            "dnf".to_string(),
            vec!["install".to_string(), "-y".to_string()],
        ),
        "pacman" => (
            "pacman".to_string(),
            vec![
                "-S".to_string(),
                "--needed".to_string(),
                "--noconfirm".to_string(),
            ],
        ),
        "brew" => ("brew".to_string(), vec!["install".to_string()]),
        _ => return None,
    };
    args.extend(packages.iter().map(|package| (*package).to_string()));
    if manager == "brew" {
        return Some(InstallPlan {
            missing_features,
            program,
            args,
        });
    }
    if current_user_is_root() {
        return Some(InstallPlan {
            missing_features,
            program,
            args,
        });
    }
    resolve_command("sudo")?;
    let mut sudo_args = vec![program];
    sudo_args.extend(args);
    Some(InstallPlan {
        missing_features,
        program: "sudo".to_string(),
        args: sudo_args,
    })
}

fn current_user_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

fn lcm_cli_program() -> String {
    non_empty_env("SIEVE_LCM_CLI_BIN").unwrap_or_else(|| "sieve-lcm-cli".to_string())
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "host/tests.rs"]
mod tests;
