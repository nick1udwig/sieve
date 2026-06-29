use super::*;
use crate::planner_products::PlannerOpaqueHandleStore;

#[test]
fn planner_step_trace_includes_opaque_handle_products_without_raw_ids() {
    let temp_path = std::env::temp_dir().join(format!(
        "sieve-planner-products-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(
        &temp_path,
        r#"{
  "messages": [
    { "id": "19cee5d38f87464f", "threadId": "19cee5d38f87464f" },
    { "id": "19cee5d280829ded", "threadId": "19cee5d280829ded" }
  ],
  "resultSizeEstimate": 2
}"#,
    )
    .expect("write artifact");

    let tool_result = PlannerToolResult::Bash {
        command: "sudo env GWS_CONFIG_DIR=/tmp/gws gws gmail users messages list --params '{\"userId\":\"me\"}'"
            .to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-products".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-gws-list".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: temp_path.display().to_string(),
                byte_count: 182,
                line_count: 6,
            }],
        }),
    };

    let mut store = PlannerOpaqueHandleStore::default();
    let products = store.record_step_products(std::slice::from_ref(&tool_result));
    let trace = planner_step_trace_messages(
        1,
        &[tool_result],
        &PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch.code(),
            confidence_bps: 9000,
            source_hit_index: None,
            evidence_ref_index: None,
        },
        &products,
    );

    assert_eq!(products.len(), 1);
    assert!(trace[1].content.contains("\"intermediate_products\""));
    assert!(trace[1]
        .content
        .contains("\"product_kind\":\"handle_list\""));
    assert!(trace[1]
        .content
        .contains("\"product_ref\":\"gws-gmail-message-1\""));
    assert!(trace[1].content.contains("\"item_count\":2"));
    assert!(!trace[1].content.contains("19cee5d38f87464f"));
    assert!(!trace[1].content.contains("19cee5d280829ded"));
    let placeholders = store.placeholder_values();
    assert_eq!(
        placeholders.get("[[handle:gws-gmail-message-1:0]]"),
        Some(&"19cee5d38f87464f".to_string())
    );

    let _ = std::fs::remove_file(temp_path);
}

#[test]
fn planner_products_ignore_failed_gws_message_list_output() {
    let temp_path = std::env::temp_dir().join(format!(
        "sieve-planner-products-failed-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(
        &temp_path,
        r#"{"messages":[{"id":"should-not-be-used"}],"resultSizeEstimate":1}"#,
    )
    .expect("write artifact");

    let tool_result = PlannerToolResult::Bash {
        command: "gws gmail users messages list --params '{\"userId\":\"me\"}'".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-products-failed".to_string()),
            exit_code: Some(1),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-gws-list-failed".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: temp_path.display().to_string(),
                byte_count: 65,
                line_count: 1,
            }],
        }),
    };

    let mut store = PlannerOpaqueHandleStore::default();
    let products = store.record_step_products(&[tool_result]);

    assert!(products.is_empty());
    assert!(store.placeholder_values().is_empty());

    let _ = std::fs::remove_file(temp_path);
}

#[test]
fn planner_products_deduplicate_gws_message_handles() {
    let temp_path = std::env::temp_dir().join(format!(
        "sieve-planner-products-dedupe-{}.json",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::write(
        &temp_path,
        r#"{"messages":[{"id":"msg-1"},{"id":"msg-1"},{"id":"msg-2"}],"resultSizeEstimate":3}"#,
    )
    .expect("write artifact");

    let tool_result = PlannerToolResult::Bash {
        command: "gws gmail users messages list --params '{\"userId\":\"me\"}'".to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-products-dedupe".to_string()),
            exit_code: Some(0),
            artifacts: vec![MainlineArtifact {
                ref_id: "artifact-gws-list-dedupe".to_string(),
                kind: MainlineArtifactKind::Stdout,
                path: temp_path.display().to_string(),
                byte_count: 83,
                line_count: 1,
            }],
        }),
    };

    let mut store = PlannerOpaqueHandleStore::default();
    let products = store.record_step_products(&[tool_result]);

    assert_eq!(products.len(), 1);
    assert_eq!(products[0].item_count, 2);
    let placeholders = store.placeholder_values();
    assert_eq!(placeholders.len(), 2);
    assert_eq!(
        placeholders.get("[[handle:gws-gmail-message-1:0]]"),
        Some(&"msg-1".to_string())
    );
    assert_eq!(
        placeholders.get("[[handle:gws-gmail-message-1:1]]"),
        Some(&"msg-2".to_string())
    );

    let _ = std::fs::remove_file(temp_path);
}

#[test]
fn planner_step_trace_includes_gws_schema_cli_shape_product() {
    let tool_result = PlannerToolResult::Bash {
        command: "sudo env GWS_CONFIG_DIR=/tmp/gws gws schema gmail.users.messages.list"
            .to_string(),
        disposition: RuntimeDisposition::ExecuteMainline(MainlineRunReport {
            run_id: RunId("run-schema".to_string()),
            exit_code: Some(0),
            artifacts: Vec::new(),
        }),
    };

    let mut store = PlannerOpaqueHandleStore::default();
    let products = store.record_step_products(std::slice::from_ref(&tool_result));
    let trace = planner_step_trace_messages(
        2,
        &[tool_result],
        &PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueNeedRequiredParameter.code(),
            confidence_bps: 9300,
            source_hit_index: None,
            evidence_ref_index: None,
        },
        &products,
    );

    assert_eq!(products.len(), 1);
    assert!(trace[1].content.contains("\"product_kind\":\"cli_shape\""));
    assert!(trace[1]
        .content
        .contains("\"product_ref\":\"gws-cli-shape-1\""));
    assert!(trace[1]
        .content
        .contains("\"command_prefix\":\"gws gmail users messages list\""));
}
