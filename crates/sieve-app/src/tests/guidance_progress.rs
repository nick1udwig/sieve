use super::*;
#[test]
fn guidance_continue_decision_auto_extends_step_limit() {
    let (should_continue, next_limit, auto_extended) = guidance_continue_decision(
        PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
        0,
        3,
        3,
        6,
    );
    assert!(should_continue);
    assert_eq!(next_limit, 4);
    assert!(auto_extended);
}

#[test]
fn guidance_continue_decision_honors_hard_limit() {
    let (should_continue, next_limit, auto_extended) = guidance_continue_decision(
        PlannerGuidanceSignal::ContinueNeedHigherQualitySource,
        0,
        6,
        6,
        6,
    );
    assert!(!should_continue);
    assert_eq!(next_limit, 6);
    assert!(!auto_extended);
}

#[test]
fn progress_contract_requires_primary_content_before_fact_ready() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: "/tmp/a".to_string(),
                byte_count: 128,
                line_count: 1,
            }],
        }),
    }];
    let override_signal = progress_contract_override_signal(
        "What is the current status?",
        PlannerGuidanceSignal::FinalSingleFactReady,
        &tool_results,
    );
    assert_eq!(
        override_signal.map(|(signal, _)| signal),
        Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch)
    );
}

#[test]
fn progress_contract_requires_non_asset_fetch_target() {
    let tool_results = vec![
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 128,
                    line_count: 1,
                }],
            }),
        },
        PlannerToolResult::Bash {
            command: "curl -sS https://imgs.search.brave.com/logo.png".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-2".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/b".to_string(),
                    byte_count: 64,
                    line_count: 1,
                }],
            }),
        },
    ];
    let override_signal = progress_contract_override_signal(
        "What is the current status?",
        PlannerGuidanceSignal::FinalAnswerReady,
        &tool_results,
    );
    assert_eq!(
        override_signal.map(|(signal, _)| signal),
        Some(PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl)
    );
}

#[test]
fn progress_contract_normalizes_time_bound_continue_to_primary_fetch() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: "/tmp/a".to_string(),
                byte_count: 128,
                line_count: 1,
            }],
        }),
    }];
    let override_signal = progress_contract_override_signal(
        "What is the current status?",
        PlannerGuidanceSignal::ContinueNeedFreshOrTimeBoundEvidence,
        &tool_results,
    );
    assert_eq!(
        override_signal.map(|(signal, _)| signal),
        Some(PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch)
    );
}

#[test]
fn progress_contract_does_not_override_hard_stop_signal() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: "/tmp/a".to_string(),
                byte_count: 128,
                line_count: 1,
            }],
        }),
    }];
    let override_signal = progress_contract_override_signal(
        "What is the current status?",
        PlannerGuidanceSignal::StopNoAllowedToolCanSatisfyTask,
        &tool_results,
    );
    assert!(override_signal.is_none());
}

#[test]
fn progress_contract_requests_higher_quality_when_fetch_output_is_too_small() {
    let tool_results = vec![
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 256,
                    line_count: 1,
                }],
            }),
        },
        PlannerToolResult::Bash {
            command: "curl -sS \"https://markdown.new/https://example.com/path?x=1\"".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-2".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/b".to_string(),
                    byte_count: 81,
                    line_count: 1,
                }],
            }),
        },
    ];
    let override_signal = progress_contract_override_signal(
        "What is the weather today?",
        PlannerGuidanceSignal::FinalAnswerReady,
        &tool_results,
    );
    assert_eq!(
        override_signal.map(|(signal, _)| signal),
        Some(PlannerGuidanceSignal::ContinueNeedHigherQualitySource)
    );
}
