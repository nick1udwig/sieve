use crate::auth::{load_provider_auth, ProviderAuth};
use crate::codex_auth::{
    default_openai_codex_auth_json_path, parse_openai_codex_authorization_input,
    read_openai_codex_auth_file, write_openai_codex_auth_file, OpenAiCodexStoredAuth,
};
use crate::config::{load_model_config_from_env, load_openai_api_key_from_env};
use crate::wire::{
    decode_guidance_output, decode_planner_output, decode_response_output,
    extract_openai_message_content_json, extract_openai_planner_output_json,
    serialize_planner_input, serialize_response_input, PlannerDecodeOutcome,
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
use std::path::PathBuf;

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
fn load_model_config_from_env_parses_openai_codex_provider() {
    let mut env_map = BTreeMap::new();
    env_map.insert(
        "SIEVE_PLANNER_MODEL".to_string(),
        "gpt-5.4-codex".to_string(),
    );
    env_map.insert(
        "SIEVE_PLANNER_PROVIDER".to_string(),
        "openai_codex".to_string(),
    );
    let cfg = load_model_config_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap();
    assert_eq!(cfg.provider, LlmProvider::OpenAiCodex);
    assert_eq!(cfg.model, "gpt-5.4-codex");
}

#[test]
fn load_model_config_from_env_falls_back_to_openai_codex_when_openai_key_missing() {
    let temp_dir = std::env::temp_dir().join(format!(
        "sieve-codex-config-fallback-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("mkdir");
    let auth_path = temp_dir.join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{
  "openai-codex": {
    "type": "oauth",
    "access": "auth-token",
    "refresh": "refresh-token",
    "expires": 4102444800000,
    "accountId": "acc-auth"
  }
}"#,
    )
    .expect("write auth");

    let mut env_map = BTreeMap::new();
    env_map.insert("SIEVE_PLANNER_MODEL".to_string(), "gpt-5.4".to_string());
    env_map.insert("SIEVE_PLANNER_PROVIDER".to_string(), "openai".to_string());
    env_map.insert(
        "SIEVE_OPENAI_CODEX_AUTH_JSON_PATH".to_string(),
        auth_path.display().to_string(),
    );

    let cfg = load_model_config_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap();
    assert_eq!(cfg.provider, LlmProvider::OpenAiCodex);

    let _ = std::fs::remove_file(auth_path);
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn load_model_config_from_env_prefers_openai_when_api_key_present() {
    let temp_dir = std::env::temp_dir().join(format!(
        "sieve-codex-config-openai-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("mkdir");
    let auth_path = temp_dir.join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{
  "openai-codex": {
    "type": "oauth",
    "access": "auth-token",
    "refresh": "refresh-token",
    "expires": 4102444800000,
    "accountId": "acc-auth"
  }
}"#,
    )
    .expect("write auth");

    let mut env_map = BTreeMap::new();
    env_map.insert("SIEVE_PLANNER_MODEL".to_string(), "gpt-5.4".to_string());
    env_map.insert("SIEVE_PLANNER_PROVIDER".to_string(), "openai".to_string());
    env_map.insert("OPENAI_API_KEY".to_string(), "openai-key".to_string());
    env_map.insert(
        "SIEVE_OPENAI_CODEX_AUTH_JSON_PATH".to_string(),
        auth_path.display().to_string(),
    );

    let cfg = load_model_config_from_env("SIEVE_PLANNER", &map_getter(&env_map)).unwrap();
    assert_eq!(cfg.provider, LlmProvider::OpenAi);

    let _ = std::fs::remove_file(auth_path);
    let _ = std::fs::remove_dir_all(temp_dir);
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
fn load_provider_auth_reads_openai_codex_env() {
    let mut env_map = BTreeMap::new();
    env_map.insert(
        "SIEVE_PLANNER_OPENAI_CODEX_ACCESS_TOKEN".to_string(),
        "token-123".to_string(),
    );
    env_map.insert(
        "SIEVE_PLANNER_OPENAI_CODEX_ACCOUNT_ID".to_string(),
        "acc-123".to_string(),
    );

    let auth = load_provider_auth(
        &["SIEVE_PLANNER"],
        LlmProvider::OpenAiCodex,
        &map_getter(&env_map),
    )
    .unwrap();

    let ProviderAuth::OpenAiCodex(auth) = auth else {
        panic!("expected openai_codex auth");
    };

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let client = reqwest::Client::builder().build().expect("client");
    let (access_token, account_id) = runtime
        .block_on(auth.access_token_and_account_id(&client))
        .expect("resolve env auth");
    assert_eq!(access_token, "token-123");
    assert_eq!(account_id, "acc-123");
}

#[test]
fn load_provider_auth_reads_openai_codex_auth_json() {
    let temp_dir = std::env::temp_dir().join(format!(
        "sieve-codex-auth-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("mkdir");
    let auth_path = temp_dir.join("auth.json");
    std::fs::write(
        &auth_path,
        r#"{
  "openai-codex": {
    "type": "oauth",
    "access": "auth-token",
    "refresh": "refresh-token",
    "expires": 4102444800000,
    "accountId": "acc-auth"
  }
}"#,
    )
    .expect("write auth");

    let mut env_map = BTreeMap::new();
    env_map.insert(
        "SIEVE_OPENAI_CODEX_AUTH_JSON_PATH".to_string(),
        auth_path.display().to_string(),
    );
    let auth = load_provider_auth(
        &["SIEVE_PLANNER"],
        LlmProvider::OpenAiCodex,
        &map_getter(&env_map),
    )
    .unwrap();

    let ProviderAuth::OpenAiCodex(auth) = auth else {
        panic!("expected openai_codex auth");
    };

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let client = reqwest::Client::builder().build().expect("client");
    let (access_token, account_id) = runtime
        .block_on(auth.access_token_and_account_id(&client))
        .expect("resolve auth file auth");
    assert_eq!(access_token, "auth-token");
    assert_eq!(account_id, "acc-auth");

    let _ = std::fs::remove_file(auth_path);
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn default_openai_codex_auth_json_path_prefers_sieve_home() {
    let path = default_openai_codex_auth_json_path(
        Some("/var/tmp/sieve-home".to_string()),
        Some("/home/alice".to_string()),
    );
    assert_eq!(path, PathBuf::from("/var/tmp/sieve-home/state/auth.json"));
}

#[test]
fn default_openai_codex_auth_json_path_falls_back_to_home() {
    let path = default_openai_codex_auth_json_path(None, Some("/home/alice".to_string()));
    assert_eq!(path, PathBuf::from("/home/alice/.sieve/state/auth.json"));
}

#[test]
fn write_openai_codex_auth_file_round_trips() {
    let temp_dir = std::env::temp_dir().join(format!(
        "sieve-codex-auth-write-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let auth_path = temp_dir.join("state/auth.json");
    let auth = OpenAiCodexStoredAuth {
        access_token: "auth-token".to_string(),
        account_id: "acc-auth".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        expires_at_ms: Some(4102444800000),
    };

    write_openai_codex_auth_file(&auth_path, &auth).expect("write auth file");
    let loaded = read_openai_codex_auth_file(&auth_path).expect("read auth file");
    assert_eq!(loaded, auth);

    let _ = std::fs::remove_file(auth_path);
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn parse_openai_codex_authorization_input_accepts_redirect_url() {
    let parsed = parse_openai_codex_authorization_input(
        "http://localhost:1455/auth/callback?code=abc123&state=state-1",
    )
    .expect("parse redirect url");
    assert_eq!(parsed.code, "abc123");
    assert_eq!(parsed.state.as_deref(), Some("state-1"));
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
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
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
fn extract_openai_message_content_json_parses_responses_output_text() {
    let response = json!({
        "output": [
            {
                "type": "message",
                "content": [
                    {
                        "type": "output_text",
                        "text": "{\"guidance\":{\"code\":200,\"confidence_bps\":9000,\"source_hit_index\":null,\"evidence_ref_index\":null}}"
                    }
                ]
            }
        ]
    });

    let content = extract_openai_message_content_json(&response).expect("parse codex output text");
    let out = decode_guidance_output(content).expect("decode guidance");
    assert_eq!(out.guidance.code, 200);
}

#[test]
fn extract_openai_message_content_json_parses_chat_completions_content_parts() {
    let response = json!({
        "choices": [
            {
                "message": {
                    "content": [
                        {
                            "type": "text",
                            "text": "{\"guidance\":{\"code\":200,\"confidence_bps\":9000,\"source_hit_index\":null,\"evidence_ref_index\":null}}"
                        }
                    ]
                }
            }
        ]
    });

    let content = extract_openai_message_content_json(&response).expect("parse content parts");
    let out = decode_guidance_output(content).expect("decode guidance");
    assert_eq!(out.guidance.code, 200);
}

#[test]
fn extract_openai_planner_output_json_parses_responses_function_calls() {
    let response = json!({
        "output": [
            {
                "type": "message",
                "content": [
                    {
                        "type": "output_text",
                        "text": "reasoning"
                    }
                ]
            },
            {
                "type": "function_call",
                "name": "bash",
                "arguments": "{\"cmd\":\"pwd\"}"
            }
        ]
    });

    let normalized = extract_openai_planner_output_json(&response).expect("normalize");
    let out = decode_planner_output(normalized).expect("decode planner");
    match out {
        PlannerDecodeOutcome::Valid(out) => {
            assert_eq!(out.tool_calls.len(), 1);
            assert_eq!(out.tool_calls[0].tool_name, "bash");
            assert_eq!(out.tool_calls[0].args["cmd"], json!("pwd"));
        }
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
            conversation: vec![sieve_types::PlannerConversationMessage {
                role: sieve_types::PlannerConversationRole::User,
                kind: sieve_types::PlannerConversationMessageKind::FullText,
                content: "Use bash to print hello world.".to_string(),
            }],
            allowed_tools: vec!["bash".to_string()],
            current_time_utc: None,
            current_timezone: None,
            allowed_net_connect_scopes: Vec::new(),
            browser_sessions: Vec::new(),
            codex_sessions: Vec::new(),
            previous_events: vec![],
            guidance: None,
        })
        .await
        .unwrap();
    assert!(out.tool_calls.iter().all(|c| c.tool_name == "bash"));
}
