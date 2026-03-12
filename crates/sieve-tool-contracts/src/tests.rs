use super::*;
use serde_json::{json, Value};
use sieve_types::{
    AutomationAction, AutomationRequest, AutomationSchedule, AutomationTarget, DeclassifyRequest,
    EndorseRequest, Integrity, SinkChannel, SinkKey, ToolContractErrorCode, ValueRef,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn validate_bash_success() {
    let call = validate("bash", &json!({ "cmd": "ls -la" })).expect("valid bash call");
    assert_eq!(
        call,
        TypedCall::Bash(BashArgs {
            cmd: "ls -la".to_string()
        })
    );
}

#[test]
fn validate_bash_requires_non_empty_cmd() {
    let err = validate("bash", &json!({ "cmd": "   " })).expect_err("empty command should fail");
    assert_eq!(err.code, ToolContractErrorCode::InvalidValue);
    assert_eq!(err.argument_path, "/cmd");
}

#[test]
fn validate_bash_rejects_unknown_field() {
    let err = validate("bash", &json!({ "cmd": "ls", "cwd": "/tmp" })).expect_err("unknown field");
    assert_eq!(err.code, ToolContractErrorCode::UnknownField);
    assert_eq!(err.argument_path, "/cwd");
}

#[test]
fn validate_unknown_tool_fails() {
    let err = validate("python", &json!({})).expect_err("unknown tool should fail");
    assert_eq!(err.code, ToolContractErrorCode::UnknownTool);
    assert_eq!(err.argument_path, "/");
}

#[test]
fn validate_rejects_non_object_args() {
    let err = validate("bash", &json!("ls -la")).expect_err("args must be object");
    assert_eq!(err.code, ToolContractErrorCode::InvalidType);
    assert_eq!(err.found.as_deref(), Some("string"));
}

#[test]
fn validate_endorse_success() {
    let call = validate(
        "endorse",
        &json!({
            "value_ref": "v123",
            "target_integrity": "trusted",
            "reason": "manual approval"
        }),
    )
    .expect("valid endorse");
    assert_eq!(
        call,
        TypedCall::Endorse(EndorseRequest {
            value_ref: ValueRef("v123".to_string()),
            target_integrity: Integrity::Trusted,
            reason: Some("manual approval".to_string()),
        })
    );
}

#[test]
fn validate_endorse_rejects_invalid_integrity_variant() {
    let err = validate(
        "endorse",
        &json!({
            "value_ref": "v123",
            "target_integrity": "high"
        }),
    )
    .expect_err("invalid integrity");
    assert_eq!(err.code, ToolContractErrorCode::InvalidEnumVariant);
    assert_eq!(err.argument_path, "/target_integrity");
}

#[test]
fn validate_declassify_success() {
    let call = validate(
        "declassify",
        &json!({
            "value_ref": "v456",
            "sink": "https://api.example.com/v1/upload",
            "channel": "body",
            "reason": null
        }),
    )
    .expect("valid declassify");
    assert_eq!(
        call,
        TypedCall::Declassify(DeclassifyRequest {
            value_ref: ValueRef("v456".to_string()),
            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: SinkChannel::Body,
            reason: None,
        })
    );
}

#[test]
fn validate_declassify_rejects_invalid_sink() {
    let err = validate(
        "declassify",
        &json!({
            "value_ref": "v456",
            "sink": "/tmp/local-file",
            "channel": "body"
        }),
    )
    .expect_err("invalid sink");
    assert_eq!(err.code, ToolContractErrorCode::InvalidValue);
    assert_eq!(err.argument_path, "/sink");
}

#[test]
fn validate_declassify_rejects_invalid_channel() {
    let err = validate(
        "declassify",
        &json!({
            "value_ref": "v456",
            "sink": "https://api.example.com/v1/upload",
            "channel": "footer"
        }),
    )
    .expect_err("invalid channel");
    assert_eq!(err.code, ToolContractErrorCode::InvalidEnumVariant);
    assert_eq!(err.argument_path, "/channel");
}

#[test]
fn validate_automation_cron_add_success() {
    let call = validate(
        "automation",
        &json!({
            "action": "cron_add",
            "target": "main",
            "schedule": {
                "kind": "at",
                "timestamp": "2026-03-08T12:34:56Z"
            },
            "prompt": "remind me to say hi"
        }),
    )
    .expect("valid automation cron_add");
    assert_eq!(
        call,
        TypedCall::Automation(AutomationRequest {
            action: AutomationAction::CronAdd,
            target: Some(AutomationTarget::Main),
            schedule: Some(AutomationSchedule::At {
                timestamp: "2026-03-08T12:34:56Z".to_string(),
            }),
            prompt: Some("remind me to say hi".to_string()),
            job_id: None,
        })
    );
}

#[test]
fn validate_automation_cron_add_requires_target() {
    let err = validate(
        "automation",
        &json!({
            "action": "cron_add",
            "schedule": {
                "kind": "after",
                "delay": "15m"
            },
            "prompt": "remind me to check deploys"
        }),
    )
    .expect_err("missing target should fail");
    assert_eq!(err.code, ToolContractErrorCode::MissingRequiredField);
    assert_eq!(err.argument_path, "/target");
}

#[test]
fn validate_automation_cron_add_after_success() {
    let call = validate(
        "automation",
        &json!({
            "action": "cron_add",
            "target": "main",
            "schedule": {
                "kind": "after",
                "delay": "1m"
            },
            "prompt": "say hi"
        }),
    )
    .expect("valid automation cron_add after");
    assert_eq!(
        call,
        TypedCall::Automation(AutomationRequest {
            action: AutomationAction::CronAdd,
            target: Some(AutomationTarget::Main),
            schedule: Some(AutomationSchedule::After {
                delay: "1m".to_string(),
            }),
            prompt: Some("say hi".to_string()),
            job_id: None,
        })
    );
}

#[test]
fn validate_at_index_sets_tool_call_index() {
    let err = validate_at_index(3, "bash", &json!({})).expect_err("missing cmd");
    assert_eq!(err.code, ToolContractErrorCode::MissingRequiredField);
    assert_eq!(err.tool_call_index, 3);
    let report = err.as_validation_error();
    assert_eq!(report.tool_call_index, 3);
}

#[test]
fn emitted_schema_documents_have_expected_keys() {
    let schemas = emitted_schema_documents();
    let keys: Vec<String> = schemas.keys().cloned().collect();
    assert_eq!(
        keys,
        vec![
            "automation-args.schema.json".to_string(),
            "bash-args.schema.json".to_string(),
            "declassify-args.schema.json".to_string(),
            "endorse-args.schema.json".to_string(),
            "planner-tool-call.schema.json".to_string(),
            "planner-turn-output.schema.json".to_string(),
        ]
    );
}

#[test]
fn planner_tool_call_schema_mentions_supported_tools() {
    let schema = planner_tool_call_schema().to_string();
    assert!(schema.contains("\"automation\""));
    assert!(schema.contains("\"bash\""));
    assert!(schema.contains("\"endorse\""));
    assert!(schema.contains("\"declassify\""));
}

#[test]
fn committed_schema_artifacts_match_generated() {
    let schema_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas");
    for (filename, expected_schema) in emitted_schema_documents() {
        let path = schema_dir.join(&filename);
        let contents = fs::read_to_string(&path).expect("read committed schema artifact");
        let committed: Value = serde_json::from_str(&contents).expect("parse committed schema");
        assert_eq!(
            committed, expected_schema,
            "schema artifact mismatch for {filename}; rerun emit-schemas"
        );
    }
}
