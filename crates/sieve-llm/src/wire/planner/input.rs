use super::PLANNER_SYSTEM_PROMPT;
use crate::LlmError;
use serde::Serialize;
use serde_json::Value;
use sieve_command_summaries::planner_available_command_catalog;
use sieve_tool_contracts::tool_descriptor;
use sieve_types::{
    PlannerBrowserSession, PlannerCodexSession, PlannerConversationMessage,
    PlannerConversationMessageKind, PlannerConversationRole, PlannerGuidanceFrame,
    PlannerGuidanceSignal, PlannerTurnInput, RuntimeEvent,
};

#[derive(Serialize)]
struct PlannerGuidancePayload {
    code: u16,
    signal_name: Option<&'static str>,
    confidence_bps: u16,
    source_hit_index: Option<u16>,
    evidence_ref_index: Option<u16>,
}

#[derive(Serialize, Default)]
struct PlannerGuidanceContract {
    #[serde(skip_serializing_if = "Option::is_none")]
    required_action_class: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forbidden_action_classes: Option<Vec<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_non_asset_target: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefer_markdown_view: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_action_change: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefer_current_browser_session: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avoid_recent_interstitial_origin: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preserve_task_target: Option<bool>,
}

#[derive(Serialize)]
struct PlannerCommandCatalogEntry<'a> {
    command: &'a str,
    description: &'a str,
}

#[derive(Serialize)]
struct PlannerToolGuideEntry<'a> {
    name: &'a str,
    family: &'a str,
    description: &'a str,
    when_to_use: &'a [&'a str],
    when_not_to_use: &'a [&'a str],
    usage_notes: &'a [&'a str],
    examples: &'a [&'a str],
}

#[derive(Serialize)]
struct PlannerContextPayload<'a> {
    run_id: &'a str,
    trusted_user_message: &'a str,
    #[serde(rename = "CURRENT_TIME_UTC")]
    current_time_utc: Option<&'a str>,
    #[serde(rename = "CURRENT_TIMEZONE")]
    current_timezone: Option<&'a str>,
    #[serde(rename = "ALLOWED_NET_CONNECT_SCOPES")]
    allowed_net_connect_scopes: &'a [String],
    #[serde(rename = "BROWSER_SESSIONS")]
    browser_sessions: &'a [PlannerBrowserSession],
    #[serde(rename = "CODEX_SESSIONS")]
    codex_sessions: &'a [PlannerCodexSession],
    #[serde(rename = "BASH_COMMAND_CATALOG")]
    bash_command_catalog: Vec<PlannerCommandCatalogEntry<'a>>,
    previous_event_kinds: Vec<&'static str>,
    guidance: Option<PlannerGuidancePayload>,
    guidance_contract: Option<PlannerGuidanceContract>,
}

#[derive(Serialize)]
struct PlannerChatMessage {
    role: &'static str,
    content: String,
}

pub(crate) fn serialize_planner_input(input: &PlannerTurnInput) -> Result<Value, LlmError> {
    if input.user_message.trim().is_empty() {
        return Err(LlmError::Boundary("empty trusted user_message".to_string()));
    }

    let event_kinds: Vec<&'static str> = input
        .previous_events
        .iter()
        .map(runtime_event_kind)
        .collect();
    let guidance = input.guidance.as_ref().map(|guidance| {
        let signal_name = PlannerGuidanceSignal::try_from(guidance.code)
            .ok()
            .map(PlannerGuidanceSignal::name);
        PlannerGuidancePayload {
            code: guidance.code,
            signal_name,
            confidence_bps: guidance.confidence_bps,
            source_hit_index: guidance.source_hit_index,
            evidence_ref_index: guidance.evidence_ref_index,
        }
    });
    let guidance_contract = input
        .guidance
        .as_ref()
        .and_then(planner_guidance_contract_payload);
    let bash_command_catalog = planner_command_catalog_for_allowed_tools(&input.allowed_tools);
    serde_json::to_value(PlannerContextPayload {
        run_id: &input.run_id.0,
        trusted_user_message: &input.user_message,
        current_time_utc: input.current_time_utc.as_deref(),
        current_timezone: input.current_timezone.as_deref(),
        allowed_net_connect_scopes: &input.allowed_net_connect_scopes,
        browser_sessions: &input.browser_sessions,
        codex_sessions: &input.codex_sessions,
        bash_command_catalog,
        previous_event_kinds: event_kinds,
        guidance,
        guidance_contract,
    })
    .map_err(|err| LlmError::Boundary(format!("failed to serialize planner input: {err}")))
}

pub(crate) fn build_planner_messages(input: &PlannerTurnInput) -> Result<Vec<Value>, LlmError> {
    let context_payload = serialize_planner_input(input)?;
    let mut messages = vec![
        planner_message("system", PLANNER_SYSTEM_PROMPT.to_string()),
        planner_message(
            "user",
            format!("TRUSTED_PLANNER_CONTEXT\n{}", context_payload),
        ),
    ];
    let tool_guide = planner_tool_guide_for_allowed_tools(&input.allowed_tools)?;
    if !tool_guide.is_empty() {
        messages.push(planner_message(
            "user",
            format!(
                "TRUSTED_TOOL_GUIDE\n{}",
                serde_json::to_string(&tool_guide).map_err(|err| {
                    LlmError::Boundary(format!("failed to serialize planner tool guide: {err}"))
                })?
            ),
        ));
    }

    if input.conversation.is_empty() {
        messages.push(planner_message("user", input.user_message.clone()));
        return Ok(messages);
    }

    for message in &input.conversation {
        messages.push(planner_message(
            planner_conversation_role(message.role),
            planner_conversation_content(message),
        ));
    }
    Ok(messages)
}

fn planner_guidance_contract_payload(
    guidance: &PlannerGuidanceFrame,
) -> Option<PlannerGuidanceContract> {
    let signal = PlannerGuidanceSignal::try_from(guidance.code).ok()?;
    match signal {
        PlannerGuidanceSignal::ContinueNeedPrimaryContentFetch => Some(PlannerGuidanceContract {
            required_action_class: Some("fetch"),
            forbidden_action_classes: Some(vec!["discovery"]),
            require_non_asset_target: Some(true),
            prefer_markdown_view: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueNeedHigherQualitySource => Some(PlannerGuidanceContract {
            required_action_class: Some("fetch"),
            forbidden_action_classes: Some(vec!["discovery"]),
            require_non_asset_target: Some(true),
            require_action_change: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueNeedCanonicalNonAssetUrl => Some(PlannerGuidanceContract {
            required_action_class: Some("fetch"),
            require_non_asset_target: Some(true),
            prefer_markdown_view: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueNeedUrlExtraction => Some(PlannerGuidanceContract {
            required_action_class: Some("extract"),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueToolDeniedTryAlternativeAllowedTool => {
            Some(PlannerGuidanceContract {
                require_action_change: Some(true),
                ..Default::default()
            })
        }
        PlannerGuidanceSignal::ContinueNoProgressTryDifferentAction => {
            Some(PlannerGuidanceContract {
                forbidden_action_classes: Some(vec!["discovery"]),
                require_action_change: Some(true),
                ..Default::default()
            })
        }
        PlannerGuidanceSignal::ContinueNeedCurrentPageInspection => Some(PlannerGuidanceContract {
            required_action_class: Some("extract"),
            forbidden_action_classes: Some(vec!["discovery"]),
            prefer_current_browser_session: Some(true),
            require_action_change: Some(true),
            ..Default::default()
        }),
        PlannerGuidanceSignal::ContinueEncounteredAccessInterstitial => {
            Some(PlannerGuidanceContract {
                require_action_change: Some(true),
                forbidden_action_classes: Some(vec!["discovery"]),
                avoid_recent_interstitial_origin: Some(true),
                preserve_task_target: Some(true),
                ..Default::default()
            })
        }
        PlannerGuidanceSignal::ContinueNeedCommandReformulation => Some(PlannerGuidanceContract {
            require_action_change: Some(true),
            preserve_task_target: Some(true),
            ..Default::default()
        }),
        _ => None,
    }
}

fn planner_conversation_role(role: PlannerConversationRole) -> &'static str {
    match role {
        PlannerConversationRole::User => "user",
        PlannerConversationRole::Assistant => "assistant",
    }
}

fn planner_conversation_content(message: &PlannerConversationMessage) -> String {
    match message.kind {
        PlannerConversationMessageKind::FullText => message.content.clone(),
        PlannerConversationMessageKind::RedactedInfo => message.content.clone(),
    }
}

fn planner_message(role: &'static str, content: String) -> Value {
    serde_json::to_value(PlannerChatMessage { role, content })
        .expect("planner message serialization should succeed")
}

fn planner_command_catalog_for_allowed_tools(
    allowed_tools: &[String],
) -> Vec<PlannerCommandCatalogEntry<'static>> {
    if !allowed_tools.iter().any(|tool| tool == "bash") {
        return Vec::new();
    }

    planner_available_command_catalog()
        .into_iter()
        .map(|descriptor| PlannerCommandCatalogEntry {
            command: descriptor.command,
            description: descriptor.description,
        })
        .collect()
}

fn planner_tool_guide_for_allowed_tools(
    allowed_tools: &[String],
) -> Result<Vec<PlannerToolGuideEntry<'static>>, LlmError> {
    let mut entries = Vec::with_capacity(allowed_tools.len());
    for tool_name in allowed_tools {
        let descriptor = tool_descriptor(tool_name).ok_or_else(|| {
            LlmError::Boundary(format!(
                "allowed tool `{tool_name}` is missing a shared descriptor"
            ))
        })?;
        entries.push(PlannerToolGuideEntry {
            name: descriptor.name,
            family: descriptor.family,
            description: descriptor.description,
            when_to_use: descriptor.when_to_use,
            when_not_to_use: descriptor.when_not_to_use,
            usage_notes: descriptor.usage_notes,
            examples: descriptor.examples,
        });
    }
    Ok(entries)
}

fn runtime_event_kind(event: &RuntimeEvent) -> &'static str {
    match event {
        RuntimeEvent::ApprovalRequested(_) => "approval_requested",
        RuntimeEvent::ApprovalResolved(_) => "approval_resolved",
        RuntimeEvent::CodexSessionStatus(_) => "codex_session_status",
        RuntimeEvent::PolicyEvaluated(_) => "policy_evaluated",
        RuntimeEvent::QuarantineCompleted(_) => "quarantine_completed",
        RuntimeEvent::AssistantMessage(_) => "assistant_message",
    }
}
