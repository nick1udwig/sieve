use super::*;
#[test]
fn has_repeated_bash_outcome_detects_duplicate_mainline_command() {
    let tool_results = vec![
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![],
            }),
        },
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![],
            }),
        },
    ];
    assert!(has_repeated_bash_outcome(&tool_results));
}

#[test]
fn has_repeated_bash_outcome_ignores_different_commands() {
    let tool_results = vec![
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::Denied {
                reason: "unknown command denied by mode".to_string(),
            },
        },
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"y\" --count 5 --output json".to_string(),
            disposition: RuntimeDisposition::Denied {
                reason: "unknown command denied by mode".to_string(),
            },
        },
    ];
    assert!(!has_repeated_bash_outcome(&tool_results));
}

#[test]
fn has_repeated_bash_outcome_detects_case_only_query_variants() {
    let tool_results = vec![
            PlannerToolResult::Bash {
                command:
                    "bravesearch search --query \"weather Livermore CA tomorrow\" --count 1 --output json"
                        .to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-1".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/a".to_string(),
                        byte_count: 2830,
                        line_count: 1,
                    }],
                }),
            },
            PlannerToolResult::Bash {
                command:
                    "bravesearch search --query \"weather Livermore ca tomorrow\" --count 1 --output json"
                        .to_string(),
                disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                    run_id: RunId("run-1".to_string()),
                    exit_code: Some(0),
                    artifacts: vec![MainlineArtifact {
                        ref_id: "artifact-2".to_string(),
                        kind: MainlineArtifactKind::Stdout,
                        path: "/tmp/b".to_string(),
                        byte_count: 2830,
                        line_count: 1,
                    }],
                }),
            },
        ];

    assert!(has_repeated_bash_outcome(&tool_results));
}

#[test]
fn has_repeated_bash_outcome_ignores_changed_artifact_signature() {
    let tool_results = vec![
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 1 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 100,
                    line_count: 1,
                }],
            }),
        },
        PlannerToolResult::Bash {
            command: "bravesearch search --query \"x\" --count 1 --output json".to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-2".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/b".to_string(),
                    byte_count: 101,
                    line_count: 1,
                }],
            }),
        },
    ];

    assert!(!has_repeated_bash_outcome(&tool_results));
}

#[test]
fn planner_policy_feedback_includes_missing_connect_denials() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "curl -sS \"https://wttr.in/Livermore,CA?format=j1\"".to_string(),
        disposition: RuntimeDisposition::Denied {
            reason: "missing capability Net:Connect:https://wttr.in/Livermore,CA".to_string(),
        },
    }];

    let feedback = planner_policy_feedback(&tool_results).expect("feedback expected");
    assert!(feedback.contains("https://wttr.in/Livermore,CA"));
    assert!(feedback.contains("markdown.new"));
    assert!(feedback.contains("curl -sS"));
}

#[test]
fn planner_policy_feedback_skips_non_connect_denials() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "bravesearch search --query \"x\" --count 1 --output json".to_string(),
        disposition: RuntimeDisposition::Denied {
            reason: "unknown command denied by mode".to_string(),
        },
    }];
    assert!(planner_policy_feedback(&tool_results).is_none());
}

#[test]
fn planner_policy_feedback_includes_markdown_raw_fallback_when_low_signal() {
    let tool_results = vec![PlannerToolResult::Bash {
            command:
                "curl -sS \"https://markdown.new/https://forecast.weather.gov/MapClick.php?lat=37.6819&lon=-121.768\""
                    .to_string(),
            disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
                run_id: RunId("run-1".to_string()),
                exit_code: Some(0),
                artifacts: vec![MainlineArtifact {
                    ref_id: "artifact-1".to_string(),
                    kind: MainlineArtifactKind::Stdout,
                    path: "/tmp/a".to_string(),
                    byte_count: 81,
                    line_count: 1,
                }],
            }),
        }];

    let feedback = planner_policy_feedback(&tool_results).expect("feedback expected");
    assert!(feedback.contains("markdown proxy fetch returned low/no usable primary content"));
    assert!(feedback.contains(
        "curl -sS \"https://forecast.weather.gov/MapClick.php?lat=37.6819&lon=-121.768\""
    ));
}

#[test]
fn planner_policy_feedback_skips_markdown_raw_fallback_when_primary_content_present() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "curl -sS \"https://markdown.new/https://example.com/article\"".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: "/tmp/a".to_string(),
                byte_count: MIN_PRIMARY_FETCH_STDOUT_BYTES,
                line_count: 5,
            }],
        }),
    }];
    assert!(planner_policy_feedback(&tool_results).is_none());
}

#[test]
fn planner_explicit_tool_feedback_reports_release_ref_for_declassify() {
    let tool_results = vec![PlannerToolResult::Declassify {
        request: sieve_types::DeclassifyRequest {
            value_ref: ValueRef("argv:5".to_string()),
            sink: sieve_types::SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: sieve_types::SinkChannel::Body,
            reason: None,
        },
        transition: Some(sieve_types::DeclassifyStateTransition {
            value_ref: ValueRef("argv:5".to_string()),
            release_value_ref: ValueRef("vrel_1".to_string()),
            sink: sieve_types::SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: sieve_types::SinkChannel::Body,
            release_value_existed: false,
            approved_by: None,
        }),
        failure_reason: None,
    }];

    let feedback = planner_explicit_tool_feedback(&tool_results).expect("feedback expected");
    assert!(feedback.contains("vrel_1"));
    assert!(feedback.contains("Use only `vrel_1` for that egress"));
    assert!(feedback.contains("Do not reuse source `argv:5`"));
}

#[test]
fn planner_explicit_tool_feedback_reports_declassify_failure() {
    let tool_results = vec![PlannerToolResult::Declassify {
        request: sieve_types::DeclassifyRequest {
            value_ref: ValueRef("v_string".to_string()),
            sink: sieve_types::SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: sieve_types::SinkChannel::Body,
            reason: None,
        },
        transition: None,
        failure_reason: Some(
            "trusted_string values require typed extraction before declassify".to_string(),
        ),
    }];

    let feedback = planner_explicit_tool_feedback(&tool_results).expect("feedback expected");
    assert!(feedback.contains("failed"));
    assert!(feedback.contains("typed extraction"));
    assert!(feedback.contains("Do not assume any release value_ref exists"));
}

#[tokio::test]
async fn planner_memory_feedback_extracts_sieve_lcm_query_payload() {
    let path = std::env::temp_dir().join(format!(
        "sieve-lcm-query-feedback-{}.json",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(
        &path,
        serde_json::json!({
            "trusted_hits": [
                {"excerpt": "You live in Livermore, California."}
            ],
            "untrusted_refs": [
                {"ref": "lcm:untrusted:summary:sum_abc"}
            ]
        })
        .to_string(),
    )
    .expect("write artifact payload");

    let tool_results = vec![PlannerToolResult::Bash {
        command: "sieve-lcm-cli query --lane both --query \"where do i live\" --json".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: path.to_string_lossy().to_string(),
                byte_count: 128,
                line_count: 1,
            }],
        }),
    }];

    let feedback = planner_memory_feedback(&tool_results)
        .await
        .expect("feedback expected");
    assert!(feedback.contains("trusted excerpt"));
    assert!(feedback.contains("Livermore"));
    assert!(feedback.contains("lcm:untrusted:summary:sum_abc"));

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn planner_memory_context_tracks_untrusted_control_refs() {
    let path = std::env::temp_dir().join(format!(
        "planner-memory-control-{}-{}.json",
        std::process::id(),
        now_ms()
    ));
    std::fs::write(
        &path,
        serde_json::json!({
            "trusted_hits": [
                {"excerpt": "The trusted note says the meeting starts at 3pm."}
            ],
            "untrusted_refs": [
                {"ref": "lcm:untrusted:summary:sum_abc"},
                {"ref": "lcm:untrusted:summary:sum_def"}
            ]
        })
        .to_string(),
    )
    .expect("write artifact payload");

    let tool_results = vec![PlannerToolResult::Bash {
        command: "sieve-lcm-cli query --lane both --query \"meeting\" --json".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: path.to_string_lossy().to_string(),
                byte_count: 196,
                line_count: 1,
            }],
        }),
    }];

    let context = planner_memory_context(&tool_results).await;
    assert!(context
        .feedback
        .as_deref()
        .is_some_and(|feedback| feedback.contains("trusted note")));
    assert_eq!(
        context.control_value_refs,
        BTreeSet::from([
            ValueRef("lcm:untrusted:summary:sum_abc".to_string()),
            ValueRef("lcm:untrusted:summary:sum_def".to_string()),
        ])
    );

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn planner_memory_feedback_ignores_non_memory_commands() {
    let tool_results = vec![PlannerToolResult::Bash {
        command: "curl -sS \"https://example.com\"".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![],
        }),
    }];

    assert!(planner_memory_feedback(&tool_results).await.is_none());
    assert!(planner_memory_context(&tool_results)
        .await
        .control_value_refs
        .is_empty());
}
