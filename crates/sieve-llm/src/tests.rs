use crate::config::{load_model_config_from_env, load_openai_api_key_from_env};
use crate::wire::{
    decode_guidance_output, decode_planner_output, decode_response_output,
    extract_openai_planner_output_json, serialize_planner_input, serialize_response_input,
    PlannerDecodeOutcome,
};
use crate::{GuidanceModel, LlmError, OpenAiGuidanceModel, OpenAiPlannerModel, PlannerModel};
use serde_json::json;
use sieve_types::{
    Action, Capability, LlmModelConfig, LlmProvider, PlannerTurnInput, PolicyDecision,
    PolicyDecisionKind, PolicyEvaluatedEvent, Resource, RunId, RuntimeEvent,
    TOOL_CONTRACTS_VERSION_V1,
};
use std::collections::BTreeMap;
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
fn load_model_config_from_env_treats_blank_api_base_as_unset() {
    let mut env_map = BTreeMap::new();
    env_map.insert("SIEVE_PLANNER_MODEL".to_string(), "gpt-4o-mini".to_string());
    env_map.insert("SIEVE_PLANNER_API_BASE".to_string(), "   ".to_string());
    let cfg = load_model_config_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap();
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
fn load_openai_api_key_from_env_falls_back_when_scoped_key_blank() {
    let mut env_map = BTreeMap::new();
    env_map.insert("SIEVE_PLANNER_OPENAI_API_KEY".to_string(), String::new());
    env_map.insert("OPENAI_API_KEY".to_string(), "fallback-key".to_string());

    let key = load_openai_api_key_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap();
    assert_eq!(key, "fallback-key");
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
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        previous_events: vec![event],
        guidance: None,
    };
    let payload = serialize_planner_input(&input).unwrap();
    let payload_string = payload.to_string();
    assert!(payload_string.contains("previous_event_kinds"));
    assert!(!payload_string.contains("contains user-provided command text"));
}

#[test]
fn decode_guidance_output_accepts_known_signal_code() {
    let raw = json!({
        "guidance": {
            "code": 200,
            "confidence_bps": 9200,
            "source_hit_index": null,
            "evidence_ref_index": 1
        }
    });
    let out = decode_guidance_output(raw).unwrap();
    assert_eq!(out.guidance.code, 200);
    assert_eq!(out.guidance.confidence_bps, 9200);
    assert_eq!(out.guidance.evidence_ref_index, Some(1));
}

#[test]
fn decode_guidance_output_rejects_unknown_signal_code() {
    let raw = json!({
        "guidance": {
            "code": 777,
            "confidence_bps": 7000,
            "source_hit_index": null,
            "evidence_ref_index": null
        }
    });
    let err = decode_guidance_output(raw).unwrap_err();
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

#[test]
fn extract_openai_planner_output_json_parses_native_tool_calls() {
    let response = json!({
        "choices": [
            {
                "message": {
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "bash",
                                "arguments": "{\"cmd\":\"ls -la\"}"
                            }
                        }
                    ]
                }
            }
        ]
    });

    let normalized = extract_openai_planner_output_json(&response).unwrap();
    let out = decode_planner_output(normalized).unwrap();
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
fn extract_openai_planner_output_json_allows_no_tool_calls() {
    let response = json!({
        "choices": [
            {
                "message": {
                    "content": "{\"thoughts\":null,\"tool_calls\":[{\"tool_name\":\"bash\",\"args\":{\"cmd\":\"pwd\"}}]}"
                }
            }
        ]
    });

    let normalized = extract_openai_planner_output_json(&response).expect("normalize");
    let out = decode_planner_output(normalized).expect("decode planner");
    match out {
        PlannerDecodeOutcome::Valid(out) => assert!(out.tool_calls.is_empty()),
        PlannerDecodeOutcome::InvalidToolContracts(_) => panic!("expected valid planner output"),
    }
}

#[test]
fn response_turn_round_trip_uses_safe_shape() {
    let payload = serialize_response_input(&crate::ResponseTurnInput {
        run_id: RunId("run-resp".to_string()),
        trusted_user_message: "hi".to_string(),
        response_modality: sieve_types::InteractionModality::Audio,
        planner_thoughts: Some("none".to_string()),
        trusted_effects: Vec::new(),
        extracted_evidence: Vec::new(),
        tool_outcomes: vec![crate::ResponseToolOutcome {
            tool_name: "bash".to_string(),
            outcome: "execute_mainline exit_code=0".to_string(),
            attempted_command: Some("pwd".to_string()),
            failure_reason: None,
            refs: vec![crate::ResponseRefMetadata {
                ref_id: "ref-1".to_string(),
                kind: "stdout".to_string(),
                byte_count: 12,
                line_count: 1,
            }],
        }],
    })
    .expect("serialize response input");
    assert!(payload.get("tool_outcomes").is_some());
    assert!(payload.to_string().contains("trusted_user_message"));
    assert!(payload.to_string().contains("response_modality"));
    assert!(payload.to_string().contains("attempted_command"));

    let out = decode_response_output(json!({
        "message": "done [[ref:ref-1]]",
        "referenced_ref_ids": ["ref-1"],
        "summarized_ref_ids": []
    }))
    .expect("decode response");
    assert_eq!(out.message, "done [[ref:ref-1]]");
    assert!(out.referenced_ref_ids.contains("ref-1"));
    assert!(out.summarized_ref_ids.is_empty());
}

#[tokio::test]
async fn openai_live_guidance_smoke_env_gated() {
    if env::var("SIEVE_RUN_OPENAI_LIVE").ok().as_deref() != Some("1") {
        return;
    }

    let api_key = match env::var("OPENAI_API_KEY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return,
    };

    let model_name = env::var("SIEVE_GUIDANCE_MODEL")
        .or_else(|_| env::var("SIEVE_PLANNER_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let model = OpenAiGuidanceModel::new(
        LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model: model_name,
            api_base: env::var("SIEVE_GUIDANCE_API_BASE")
                .ok()
                .or_else(|| env::var("SIEVE_PLANNER_API_BASE").ok()),
        },
        api_key,
    )
    .unwrap();

    let input = sieve_types::PlannerGuidanceInput {
        run_id: RunId("live-smoke".to_string()),
        prompt: "User said hello. No tool output exists. Prefer final answer ready.".to_string(),
    };
    let out = model.classify_guidance(input).await.unwrap();
    assert!(out.guidance.code > 0);
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
            current_time_utc: None,
            current_timezone: None,
            allowed_net_connect_scopes: Vec::new(),
            browser_sessions: Vec::new(),
            previous_events: vec![],
            guidance: None,
        })
        .await
        .unwrap();
    assert!(out.tool_calls.iter().all(|c| c.tool_name == "bash"));
}
