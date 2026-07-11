use super::*;

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
