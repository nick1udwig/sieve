use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Default)]
struct FakeHostProbe {
    os: &'static str,
    architecture: &'static str,
    env: BTreeMap<String, String>,
    commands: BTreeMap<String, PathBuf>,
    root: bool,
}

impl HostProbe for FakeHostProbe {
    fn os(&self) -> &str {
        self.os
    }

    fn architecture(&self) -> &str {
        self.architecture
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn resolve_command(&self, command: &str) -> Option<PathBuf> {
        self.commands.get(command).cloned()
    }

    fn current_user_is_root(&self) -> bool {
        self.root
    }
}

#[test]
fn doctor_report_contains_stable_capability_ids() {
    let report = HostReport::detect();
    let ids = report
        .capabilities
        .iter()
        .map(|capability| capability.id)
        .collect::<Vec<_>>();
    assert!(ids.contains(&"shell"));
    assert!(ids.contains(&"quarantine"));
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"model_auth"));
    assert!(ids.contains(&"model_config"));
}

#[test]
fn package_plan_uses_typed_package_manager_arguments() {
    let plan = package_manager_install_plan("apt-get", &["bash", "ripgrep"], vec!["bash", "rg"]);
    if current_user_is_root() {
        assert_eq!(
            plan.expect("plan").display_command(),
            "apt-get install -y bash ripgrep"
        );
    }
}

#[test]
fn brew_plan_does_not_request_sudo() {
    let plan = package_manager_install_plan("brew", &["ripgrep"], vec!["rg"]).expect("brew plan");
    assert_eq!(plan.display_command(), "brew install ripgrep");
}

#[test]
fn quarantine_is_unavailable_off_linux_even_when_commands_exist() {
    let path = |command: &str| PathBuf::from(format!("/bin/{command}"));
    let probe = FakeHostProbe {
        os: "macos",
        architecture: "aarch64",
        commands: BTreeMap::from([
            ("bash".to_string(), path("bash")),
            ("bwrap".to_string(), path("bwrap")),
            ("strace".to_string(), path("strace")),
        ]),
        ..Default::default()
    };

    let manifest = RuntimeCapabilityManifest::detect_with(&probe);

    assert!(!manifest.quarantine_available());
    let report = manifest.report();
    let quarantine = report
        .capabilities
        .iter()
        .find(|capability| capability.id == "quarantine")
        .expect("quarantine capability");
    assert_eq!(quarantine.status, CapabilityStatus::Unsupported);
}

#[test]
fn configured_lcm_binary_is_advertised_to_the_planner() {
    let configured = "/opt/sieve/bin/sieve-lcm-cli";
    let probe = FakeHostProbe {
        os: "linux",
        architecture: "x86_64",
        env: BTreeMap::from([("SIEVE_LCM_CLI_BIN".to_string(), configured.to_string())]),
        commands: BTreeMap::from([(configured.to_string(), PathBuf::from(configured))]),
        ..Default::default()
    };

    let manifest = RuntimeCapabilityManifest::detect_with(&probe);

    let lcm = manifest
        .planner_commands()
        .iter()
        .find(|command| command.command == configured)
        .expect("configured LCM planner command");
    assert!(lcm.description.contains(configured));
}
