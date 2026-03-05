use super::*;
use sieve_types::RunId;

#[test]
fn planner_prompt_mentions_markdown_new_fetch_strategy() {
    assert!(PLANNER_SYSTEM_PROMPT.contains("markdown.new"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("discovery/search output"));
}

#[test]
fn guidance_prompt_prefers_continue_for_discovery_only_evidence() {
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("discovery/search snippets"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("prefer continue"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("110 continue_need_primary_content_fetch"));
}

#[test]
fn serialize_planner_input_includes_bash_command_catalog_when_bash_allowed() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "search for rust async docs".to_string(),
        allowed_tools: vec!["bash".to_string()],
        allowed_net_connect_scopes: vec!["https://api.open-meteo.com".to_string()],
        previous_events: Vec::new(),
        guidance: None,
    })
    .expect("serialize planner input");

    let net_scopes = payload
        .pointer("/ALLOWED_NET_CONNECT_SCOPES")
        .and_then(Value::as_array)
        .expect("net connect scopes array");
    assert_eq!(net_scopes.len(), 1);
    assert_eq!(net_scopes[0].as_str(), Some("https://api.open-meteo.com"));

    let catalog = payload
        .pointer("/BASH_COMMAND_CATALOG")
        .and_then(Value::as_array)
        .expect("bash command catalog array");
    assert!(!catalog.is_empty(), "catalog should not be empty");
    assert!(
        payload.pointer("/ALLOWED_TOOLS").is_none(),
        "tool availability is enforced via tool-calling schema, not duplicated in prompt JSON"
    );
    assert!(catalog
        .iter()
        .any(|entry| { entry.get("command").and_then(Value::as_str) == Some("bravesearch") }));
}

#[test]
fn serialize_planner_input_omits_bash_command_catalog_when_bash_disallowed() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "mark value trusted".to_string(),
        allowed_tools: vec!["endorse".to_string(), "declassify".to_string()],
        allowed_net_connect_scopes: Vec::new(),
        previous_events: Vec::new(),
        guidance: None,
    })
    .expect("serialize planner input");

    let catalog = payload
        .pointer("/BASH_COMMAND_CATALOG")
        .and_then(Value::as_array)
        .expect("bash command catalog array");
    assert!(catalog.is_empty(), "catalog should be empty");
}

#[test]
fn serialize_planner_input_includes_guidance_contract_for_fetch_signal() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "latest weather".to_string(),
        allowed_tools: vec!["bash".to_string()],
        allowed_net_connect_scopes: Vec::new(),
        previous_events: Vec::new(),
        guidance: Some(sieve_types::PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch.code(),
            confidence_bps: 9000,
            source_hit_index: None,
            evidence_ref_index: None,
        }),
    })
    .expect("serialize planner input");

    assert_eq!(
        payload
            .pointer("/guidance_contract/required_action_class")
            .and_then(Value::as_str),
        Some("fetch")
    );
    assert_eq!(
        payload
            .pointer("/guidance/signal_name")
            .and_then(Value::as_str),
        Some("continue_need_primary_content_fetch")
    );
    assert_eq!(
        payload
            .pointer("/guidance_contract/prefer_markdown_view")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn serialize_planner_input_includes_action_change_contract_for_denied_tool_signal() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "status".to_string(),
        allowed_tools: vec!["bash".to_string()],
        allowed_net_connect_scopes: Vec::new(),
        previous_events: Vec::new(),
        guidance: Some(sieve_types::PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool.code(),
            confidence_bps: 9000,
            source_hit_index: None,
            evidence_ref_index: None,
        }),
    })
    .expect("serialize planner input");

    assert_eq!(
        payload
            .pointer("/guidance_contract/require_action_change")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn serialize_planner_input_includes_fetch_contract_for_higher_quality_signal() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "status".to_string(),
        allowed_tools: vec!["bash".to_string()],
        allowed_net_connect_scopes: Vec::new(),
        previous_events: Vec::new(),
        guidance: Some(sieve_types::PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueNeedHigherQualitySource.code(),
            confidence_bps: 9000,
            source_hit_index: None,
            evidence_ref_index: None,
        }),
    })
    .expect("serialize planner input");

    assert_eq!(
        payload
            .pointer("/guidance_contract/required_action_class")
            .and_then(Value::as_str),
        Some("fetch")
    );
    assert_eq!(
        payload
            .pointer("/guidance_contract/require_action_change")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert!(
        payload
            .pointer("/guidance_contract/prefer_markdown_view")
            .is_none(),
        "higher-quality retry should allow raw-url fallback when markdown proxy underperforms"
    );
}
