use super::*;

#[test]
fn planner_step_trace_redacts_raw_artifact_contents() {
    let temp_path = std::env::temp_dir().join(format!(
        "sieve-planner-trace-{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(&temp_path, "secret raw output").expect("write artifact");
    let tool_result = PlannerToolResult::Bash {
        command: "gws gmail users messages list --params '{\"userId\":\"me\"}'".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-1".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-1".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: temp_path.display().to_string(),
                byte_count: "secret raw output".len() as u64,
                line_count: 1,
            }],
        }),
    };

    let trace = planner_step_trace_messages(
        1,
        &[tool_result],
        &PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch.code(),
            confidence_bps: 9000,
            source_hit_index: None,
            evidence_ref_index: None,
        },
        &[],
    );

    assert_eq!(trace.len(), 2);
    assert!(trace[0].content.contains("TRUSTED_PLANNER_ACTIONS"));
    assert!(trace[1]
        .content
        .contains("TRUSTED_REDACTED_STEP_OBSERVATION"));
    assert!(trace[1].content.contains("\"stdout_bytes\":17"));
    assert!(!trace[1].content.contains("secret raw output"));

    let _ = std::fs::remove_file(temp_path);
}
