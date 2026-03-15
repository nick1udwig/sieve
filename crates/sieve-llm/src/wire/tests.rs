use super::*;
use sieve_types::RunId;

#[test]
fn build_planner_messages_uses_context_and_conversation() {
    let messages = build_planner_messages(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "current question".to_string(),
        conversation: vec![
            sieve_types::PlannerConversationMessage {
                role: sieve_types::PlannerConversationRole::User,
                kind: sieve_types::PlannerConversationMessageKind::FullText,
                content: "earlier user turn".to_string(),
            },
            sieve_types::PlannerConversationMessage {
                role: sieve_types::PlannerConversationRole::Assistant,
                kind: sieve_types::PlannerConversationMessageKind::RedactedInfo,
                content: "TRUSTED_REDACTED_STEP_OBSERVATION\n{\"step_index\":1}".to_string(),
            },
        ],
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
        previous_events: Vec::new(),
        guidance: None,
    })
    .expect("build planner messages");

    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    assert!(messages[1]["content"]
        .as_str()
        .expect("context string")
        .contains("TRUSTED_PLANNER_CONTEXT"));
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"], "earlier user turn");
    assert_eq!(messages[3]["role"], "assistant");
    assert!(messages[3]["content"]
        .as_str()
        .expect("redacted string")
        .contains("TRUSTED_REDACTED_STEP_OBSERVATION"));
}

#[test]
fn planner_prompt_mentions_markdown_new_fetch_strategy() {
    assert!(PLANNER_SYSTEM_PROMPT.contains("markdown.new"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("discovery/search output"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("BROWSER_SESSIONS"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("CODEX_SESSIONS"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("TRUSTED_PLANNER_CONTEXT"));
}

#[test]
fn planner_prompt_mentions_gws_schema_to_cli_mapping() {
    assert!(PLANNER_SYSTEM_PROMPT.contains("gws schema gmail.users.messages.list"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("gws gmail users messages list"));
    assert!(PLANNER_SYSTEM_PROMPT.contains("Never emit dotted GWS subcommands"));
}

#[test]
fn guidance_prompt_prefers_continue_for_discovery_only_evidence() {
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("discovery/search snippets"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("prefer continue"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("110 continue_need_primary_content_fetch"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("114 continue_need_current_page_inspection"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("115 continue_encountered_access_interstitial"));
    assert!(GUIDANCE_SYSTEM_PROMPT.contains("116 continue_need_command_reformulation"));
}

#[test]
fn serialize_planner_input_includes_bash_command_catalog_when_bash_allowed() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "search for rust async docs".to_string(),
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: Some("2026-03-08T06:30:00Z".to_string()),
        current_timezone: Some("UTC".to_string()),
        allowed_net_connect_scopes: vec!["https://api.open-meteo.com".to_string()],
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
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
    let browser_sessions = payload
        .pointer("/BROWSER_SESSIONS")
        .and_then(Value::as_array)
        .expect("browser sessions array");
    assert!(browser_sessions.is_empty());
    assert_eq!(
        payload.pointer("/CURRENT_TIME_UTC").and_then(Value::as_str),
        Some("2026-03-08T06:30:00Z")
    );
    assert_eq!(
        payload.pointer("/CURRENT_TIMEZONE").and_then(Value::as_str),
        Some("UTC")
    );

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
        conversation: Vec::new(),
        allowed_tools: vec!["endorse".to_string(), "declassify".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
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
fn serialize_planner_input_includes_browser_sessions() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "inspect the page".to_string(),
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: vec![sieve_types::PlannerBrowserSession {
            session_name: "ytsearch".to_string(),
            current_origin: "https://www.youtube.com".to_string(),
            current_url: "https://www.youtube.com/results?search_query=jordan+peterson".to_string(),
        }],
        codex_sessions: Vec::new(),
        previous_events: Vec::new(),
        guidance: None,
    })
    .expect("serialize planner input");

    assert_eq!(
        payload
            .pointer("/BROWSER_SESSIONS/0/session_name")
            .and_then(Value::as_str),
        Some("ytsearch")
    );
}

#[test]
fn serialize_planner_input_includes_guidance_contract_for_fetch_signal() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "latest weather".to_string(),
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
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
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
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
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: Vec::new(),
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

#[test]
fn serialize_planner_input_includes_browser_inspection_contract() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "what is the top video".to_string(),
        conversation: Vec::new(),
        allowed_tools: vec!["bash".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: vec![sieve_types::PlannerBrowserSession {
            session_name: "ytsearch".to_string(),
            current_origin: "https://www.youtube.com".to_string(),
            current_url: "https://www.youtube.com/results?search_query=jordan+peterson".to_string(),
        }],
        codex_sessions: Vec::new(),
        previous_events: Vec::new(),
        guidance: Some(sieve_types::PlannerGuidanceFrame {
            code: PlannerGuidanceSignal::ContinueNeedCurrentPageInspection.code(),
            confidence_bps: 9000,
            source_hit_index: None,
            evidence_ref_index: Some(0),
        }),
    })
    .expect("serialize planner input");

    assert_eq!(
        payload
            .pointer("/guidance_contract/prefer_current_browser_session")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        payload
            .pointer("/guidance_contract/required_action_class")
            .and_then(Value::as_str),
        Some("extract")
    );
}

#[test]
fn serialize_planner_input_includes_codex_sessions() {
    let payload = serialize_planner_input(&PlannerTurnInput {
        run_id: RunId("run-1".to_string()),
        user_message: "continue the implementation".to_string(),
        conversation: Vec::new(),
        allowed_tools: vec!["codex_session".to_string()],
        current_time_utc: None,
        current_timezone: None,
        allowed_net_connect_scopes: Vec::new(),
        browser_sessions: Vec::new(),
        codex_sessions: vec![sieve_types::PlannerCodexSession {
            session_id: "fix-auth-flow".to_string(),
            session_name: "fix-auth-flow".to_string(),
            cwd: "/tmp/repo".to_string(),
            sandbox: sieve_types::CodexSandboxMode::WorkspaceWrite,
            updated_at_utc: "2026-03-09T12:00:00Z".to_string(),
            status: "completed".to_string(),
            task_summary: "fix auth flow tests".to_string(),
            last_result_summary: Some("implemented parser changes".to_string()),
        }],
        previous_events: Vec::new(),
        guidance: None,
    })
    .expect("serialize planner input");

    assert_eq!(
        payload
            .pointer("/CODEX_SESSIONS/0/session_id")
            .and_then(Value::as_str),
        Some("fix-auth-flow")
    );
}
