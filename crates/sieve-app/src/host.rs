use serde::Serialize;
use std::process::Command;

mod manifest;

#[cfg(test)]
use manifest::HostProbe;
pub(crate) use manifest::RuntimeCapabilityManifest;

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
        RuntimeCapabilityManifest::current().report()
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

pub(crate) fn quarantine_available() -> bool {
    RuntimeCapabilityManifest::current().quarantine_available()
}

pub(crate) fn codex_available() -> bool {
    RuntimeCapabilityManifest::current().codex_available()
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

#[cfg(test)]
fn package_manager_install_plan(
    manager: &str,
    packages: &[&str],
    missing_features: Vec<&'static str>,
) -> Option<InstallPlan> {
    package_manager_install_plan_with_privilege(
        manager,
        packages,
        missing_features,
        current_user_is_root(),
        sieve_command_summaries::command_available("sudo"),
    )
}

fn package_manager_install_plan_with_privilege(
    manager: &str,
    packages: &[&str],
    missing_features: Vec<&'static str>,
    current_user_is_root: bool,
    sudo_available: bool,
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
    if current_user_is_root {
        return Some(InstallPlan {
            missing_features,
            program,
            args,
        });
    }
    if !sudo_available {
        return None;
    }
    let mut sudo_args = vec![program];
    sudo_args.extend(args);
    Some(InstallPlan {
        missing_features,
        program: "sudo".to_string(),
        args: sudo_args,
    })
}

#[cfg(test)]
fn current_user_is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

#[cfg(test)]
#[path = "host/tests.rs"]
mod tests;
