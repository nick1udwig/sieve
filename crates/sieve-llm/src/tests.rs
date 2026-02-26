use crate::config::load_model_config_from_env;
use crate::wire::{
    decode_planner_output, decode_quarantine_output, serialize_planner_input, PlannerDecodeOutcome,
};
use crate::{LlmError, OpenAiPlannerModel, OpenAiQuarantineModel, PlannerModel, QuarantineModel};
use serde_json::json;
use sieve_types::{
    Action, Capability, LlmModelConfig, LlmProvider, PlannerTurnInput, PolicyDecision,
    PolicyDecisionKind, PolicyEvaluatedEvent, QuarantineExtractInput, Resource, RunId,
    RuntimeEvent, TypedValue, TOOL_CONTRACTS_VERSION_V1,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;

fn map_getter<'a>(map: &'a BTreeMap<String, String>) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| map.get(key).cloned()
}

#[test]
fn load_model_config_from_env_parses_defaults() {
    let mut env_map = BTreeMap::new();
    env_map.insert("SIEVE_PLANNER_MODEL".to_string(), "gpt-4o-mini".to_string());
    let cfg = load_model_config_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap();
    assert_eq!(cfg.provider, LlmProvider::OpenAi);
    assert_eq!(cfg.model, "gpt-4o-mini");
    assert_eq!(cfg.api_base, None);
}

#[test]
fn load_model_config_from_env_rejects_unsupported_provider() {
    let mut env_map = BTreeMap::new();
    env_map.insert("SIEVE_PLANNER_MODEL".to_string(), "gpt-4o-mini".to_string());
    env_map.insert(
        "SIEVE_PLANNER_PROVIDER".to_string(),
        "anthropic".to_string(),
    );
    let err = load_model_config_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap_err();
    assert!(matches!(err, LlmError::Config(_)));
}

#[test]
fn serialize_planner_input_only_sends_safe_shape() {
    let event = RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
        schema_version: 1,
        run_id: RunId("run-1".to_string()),
        decision: PolicyDecision {
            kind: PolicyDecisionKind::DenyWithApproval,
            reason: "contains user-provided command text".to_string(),
            blocked_rule_id: Some("rule-7".to_string()),
        },
        inferred_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "/tmp/x".to_string(),
        }],
        trace_path: None,
        created_at_ms: 0,
    });
    let input = PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "list files".to_string(),
        allowed_tools: vec!["bash".to_string()],
        previous_events: vec![event],
    };
    let payload = serialize_planner_input(&input).unwrap();
    let payload_string = payload.to_string();
    assert!(payload_string.contains("previous_event_kinds"));
    assert!(!payload_string.contains("contains user-provided command text"));
}

#[test]
fn decode_quarantine_output_validates_enum_registry() {
    let mut reg = BTreeMap::new();
    reg.insert(
        "risk".to_string(),
        BTreeSet::from(["low".to_string(), "high".to_string()]),
    );
    let raw = json!({
        "value": {
            "type":"enum",
            "value":{"registry":"risk","variant":"high"}
        }
    });
    let out = decode_quarantine_output(raw, &reg).unwrap();
    assert_eq!(
        out.value,
        TypedValue::Enum {
            registry: "risk".to_string(),
            variant: "high".to_string()
        }
    );
}

#[test]
fn decode_quarantine_output_rejects_unknown_variant() {
    let mut reg = BTreeMap::new();
    reg.insert(
        "risk".to_string(),
        BTreeSet::from(["low".to_string(), "high".to_string()]),
    );
    let raw = json!({
        "value": {
            "type":"enum",
            "value":{"registry":"risk","variant":"critical"}
        }
    });
    let err = decode_quarantine_output(raw, &reg).unwrap_err();
    assert!(matches!(err, LlmError::Boundary(_)));
}

#[test]
fn decode_planner_output_parses_tool_args() {
    let raw = json!({
        "thoughts": null,
        "tool_calls": [
            {"tool_name":"bash","args":{"cmd":"ls -la"}}
        ]
    });
    let out = decode_planner_output(raw).unwrap();
    match out {
        PlannerDecodeOutcome::Valid(out) => {
            assert_eq!(out.tool_calls.len(), 1);
            assert_eq!(out.tool_calls[0].tool_name, "bash");
            assert_eq!(out.tool_calls[0].args["cmd"], json!("ls -la"));
        }
        PlannerDecodeOutcome::InvalidToolContracts(_) => panic!("expected valid planner output"),
    }
}

#[test]
fn decode_planner_output_returns_diagnostics_for_contract_failure() {
    let raw = json!({
        "thoughts": null,
        "tool_calls": [
            {"tool_name":"bash","args":{"cmd":123}}
        ]
    });
    let out = decode_planner_output(raw).unwrap();
    let report = match out {
        PlannerDecodeOutcome::InvalidToolContracts(report) => report,
        PlannerDecodeOutcome::Valid(_) => panic!("expected tool-contract diagnostics"),
    };

    assert_eq!(report.contract_version, TOOL_CONTRACTS_VERSION_V1);
    assert_eq!(report.errors.len(), 1);
    let err = &report.errors[0];
    assert_eq!(err.tool_call_index, 0);
    assert_eq!(err.tool_name, "bash");
    assert_eq!(err.argument_path, "/cmd");
    assert!(err.hint.is_some());
    assert!(err.span.is_some());
}

#[tokio::test]
async fn openai_live_quarantine_smoke_env_gated() {
    if env::var("SIEVE_RUN_OPENAI_LIVE").ok().as_deref() != Some("1") {
        return;
    }

    let api_key = match env::var("OPENAI_API_KEY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };

    let model_name =
        env::var("SIEVE_QUARANTINE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let model = OpenAiQuarantineModel::new(
        LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model: model_name,
            api_base: env::var("SIEVE_QUARANTINE_API_BASE").ok(),
        },
        api_key,
    )
    .unwrap();

    let input = QuarantineExtractInput {
        run_id: RunId("live-smoke".to_string()),
        prompt: "Return boolean true.".to_string(),
        enum_registry: BTreeMap::new(),
    };
    let out = model.extract_typed(input).await.unwrap();
    assert_eq!(out.value, TypedValue::Bool(true));
}

#[tokio::test]
async fn openai_live_planner_smoke_env_gated() {
    if env::var("SIEVE_RUN_OPENAI_LIVE").ok().as_deref() != Some("1") {
        return;
    }

    let api_key = match env::var("OPENAI_API_KEY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };

    let model_name = env::var("SIEVE_PLANNER_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let model = OpenAiPlannerModel::new(
        LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model: model_name,
            api_base: env::var("SIEVE_PLANNER_API_BASE").ok(),
        },
        api_key,
    )
    .unwrap();

    let out = model
        .plan_turn(PlannerTurnInput {
            run_id: RunId("live-planner".to_string()),
            user_message: "Use bash to print hello world.".to_string(),
            allowed_tools: vec!["bash".to_string()],
            previous_events: vec![],
        })
        .await
        .unwrap();
    assert!(out.tool_calls.iter().all(|c| c.tool_name == "bash"));
}
