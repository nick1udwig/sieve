#![forbid(unsafe_code)]

use serde_json::Value;
use sieve_lcm::compaction::CompactionConfig;
use sieve_lcm::engine::AssembleResult;
use sieve_lcm::planner_context::{OpaqueContextRef, PlannerLaneMemory};
use sieve_lcm::store::conversation_store::MessageRole;
use sieve_lcm::summarize::{LcmSummarizeFn, LcmSummarizeOptions};
use sieve_lcm::types::AgentMessage;
use sieve_llm::{SummaryModel, SummaryRequest};
use sieve_types::{
    PlannerConversationMessage, PlannerConversationMessageKind, PlannerConversationRole, RunId,
};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

const DEFAULT_LCM_CONTEXT_TOKEN_BUDGET: i64 = 128_000;
const DEFAULT_LCM_OPAQUE_REF_TOKEN_BUDGET: i64 = 8_000;

#[derive(Clone)]
pub struct LcmIntegrationConfig {
    pub enabled: bool,
    pub global_session_id: String,
    pub trusted_db_path: PathBuf,
    pub untrusted_db_path: PathBuf,
}

impl LcmIntegrationConfig {
    pub fn from_sieve_home(sieve_home: &Path) -> Self {
        let trusted_db_path = env::var("SIEVE_LCM_TRUSTED_DB_PATH")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| sieve_home.join("lcm/trusted.db"));
        let untrusted_db_path = env::var("SIEVE_LCM_UNTRUSTED_DB_PATH")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| sieve_home.join("lcm/untrusted.db"));
        let global_session_id = env::var("SIEVE_LCM_GLOBAL_SESSION_ID")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| "global".to_string());
        Self {
            enabled: parse_bool_env("SIEVE_LCM_ENABLED", true),
            global_session_id,
            trusted_db_path,
            untrusted_db_path,
        }
    }
}

#[derive(Clone)]
pub struct LcmIntegration {
    config: LcmIntegrationConfig,
    trusted_lane: PlannerLaneMemory,
    untrusted_lane: PlannerLaneMemory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannerMemoryContext {
    pub messages: Vec<PlannerConversationMessage>,
}

impl LcmIntegration {
    pub fn new(config: LcmIntegrationConfig) -> Result<Self, String> {
        if !config.enabled {
            return Err("lcm integration disabled".to_string());
        }
        let compaction_config = compaction_config();
        let trusted_lane = PlannerLaneMemory::open(
            &config.trusted_db_path.to_string_lossy(),
            compaction_config.clone(),
            Some("UTC".to_string()),
        )
        .map_err(|err| format!("open trusted lcm lane failed: {err}"))?;
        let untrusted_lane = PlannerLaneMemory::open(
            &config.untrusted_db_path.to_string_lossy(),
            compaction_config,
            Some("UTC".to_string()),
        )
        .map_err(|err| format!("open untrusted lcm lane failed: {err}"))?;
        Ok(Self {
            config,
            trusted_lane,
            untrusted_lane,
        })
    }

    pub async fn ingest_user_message_for_session(
        &self,
        session_key: &str,
        message: &str,
    ) -> Result<(), String> {
        if message.trim().is_empty() {
            return Ok(());
        }

        let conversation = self.conversation_id_for_session(session_key);
        self.trusted_lane
            .ingest_text_message(&conversation, MessageRole::User, message)
            .map_err(|err| format!("trusted lcm ingest failed: {err}"))?;
        self.untrusted_lane
            .ingest_text_message(&conversation, MessageRole::User, message)
            .map_err(|err| format!("untrusted lcm ingest failed: {err}"))?;
        Ok(())
    }

    pub async fn ingest_assistant_message_for_session(
        &self,
        session_key: &str,
        message: &str,
    ) -> Result<(), String> {
        if message.trim().is_empty() {
            return Ok(());
        }

        let conversation = self.conversation_id_for_session(session_key);
        self.untrusted_lane
            .ingest_text_message(&conversation, MessageRole::Assistant, message)
            .map_err(|err| format!("untrusted lcm assistant ingest failed: {err}"))?;
        Ok(())
    }

    pub async fn compact_session_for_turn(
        &self,
        session_key: &str,
        run_id: &RunId,
        summary_model: Arc<dyn SummaryModel>,
        token_budget: Option<i64>,
    ) -> Result<(), String> {
        let conversation = self.conversation_id_for_session(session_key);
        let summarize_trusted =
            build_compaction_summarize(run_id, "trusted", summary_model.clone());
        let summarize_untrusted = build_compaction_summarize(run_id, "untrusted", summary_model);
        let token_budget = token_budget.unwrap_or(DEFAULT_LCM_CONTEXT_TOKEN_BUDGET);

        self.trusted_lane
            .compact_session(&conversation, token_budget, summarize_trusted)
            .await
            .map_err(|err| format!("trusted lcm compact failed: {err}"))?;
        self.untrusted_lane
            .compact_session(&conversation, token_budget, summarize_untrusted)
            .await
            .map_err(|err| format!("untrusted lcm compact failed: {err}"))?;
        Ok(())
    }

    pub async fn planner_context_for_session(
        &self,
        session_key: &str,
        token_budget: Option<i64>,
    ) -> Result<PlannerMemoryContext, String> {
        let conversation = self.conversation_id_for_session(session_key);
        let trusted = self
            .trusted_lane
            .assemble_trusted_context(
                &conversation,
                token_budget.unwrap_or(DEFAULT_LCM_CONTEXT_TOKEN_BUDGET),
            )
            .await
            .map_err(|err| format!("assemble trusted lcm context failed: {err}"))?;
        let untrusted_refs = self
            .untrusted_lane
            .assemble_opaque_refs(&conversation, DEFAULT_LCM_OPAQUE_REF_TOKEN_BUDGET)
            .map_err(|err| format!("assemble untrusted lcm refs failed: {err}"))?;

        Ok(PlannerMemoryContext {
            messages: planner_context_messages(trusted, untrusted_refs),
        })
    }

    fn conversation_id_for_session(&self, session_key: &str) -> String {
        let trimmed = session_key.trim();
        if trimmed.is_empty() || trimmed == "main" {
            return self.config.global_session_id.clone();
        }
        format!("{}:{trimmed}", self.config.global_session_id)
    }
}

fn parse_bool_env(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn compaction_config() -> CompactionConfig {
    CompactionConfig {
        context_threshold: 0.75,
        fresh_tail_count: 8,
        leaf_min_fanout: 8,
        condensed_min_fanout: 4,
        condensed_min_fanout_hard: 2,
        incremental_max_depth: 0,
        leaf_chunk_tokens: 20_000,
        leaf_target_tokens: 600,
        condensed_target_tokens: 900,
        max_rounds: 10,
        timezone: Some("UTC".to_string()),
    }
}

fn build_compaction_summarize(
    run_id: &RunId,
    lane: &'static str,
    summary_model: Arc<dyn SummaryModel>,
) -> LcmSummarizeFn {
    let run_id = run_id.clone();
    let model = summary_model.clone();
    Arc::new(move |text, aggressive, options| {
        let request = SummaryRequest {
            run_id: run_id.clone(),
            ref_id: format!("lcm-compaction:{lane}:{}", Uuid::new_v4()),
            content: render_compaction_request(&text, aggressive, options.as_ref()),
            byte_count: text.as_bytes().len() as u64,
            line_count: text.lines().count() as u64,
        };
        let model = model.clone();
        Box::pin(async move {
            match model.as_ref().summarize_ref(request).await {
                Ok(summary) => summary.trim().to_string(),
                Err(_) => deterministic_compaction_fallback(&text),
            }
        })
    })
}

fn render_compaction_request(
    text: &str,
    aggressive: bool,
    options: Option<&LcmSummarizeOptions>,
) -> String {
    let mut sections = vec![
        "task=lcm_compaction".to_string(),
        format!("aggressive={aggressive}"),
        format!(
            "is_condensed={}",
            options
                .and_then(|value| value.is_condensed)
                .unwrap_or(false)
        ),
    ];
    if let Some(depth) = options.and_then(|value| value.depth) {
        sections.push(format!("depth={depth}"));
    }
    if let Some(previous) = options
        .and_then(|value| value.previous_summary.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!("previous_summary:\n{previous}"));
    }
    sections.push("source:\n".to_string() + text);
    sections.join("\n\n")
}

fn deterministic_compaction_fallback(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "[empty compaction source]".to_string();
    }
    trimmed
        .chars()
        .take(2_048)
        .collect::<String>()
        .trim()
        .to_string()
}

fn planner_context_messages(
    trusted: AssembleResult,
    untrusted_refs: Vec<OpaqueContextRef>,
) -> Vec<PlannerConversationMessage> {
    let mut messages = Vec::new();
    if let Some(addition) = trusted
        .system_prompt_addition
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        messages.push(PlannerConversationMessage {
            role: PlannerConversationRole::User,
            kind: PlannerConversationMessageKind::RedactedInfo,
            content: format!("TRUSTED_LCM_SYSTEM_PROMPT_ADDITION\n{addition}"),
        });
    }
    messages.extend(
        trusted
            .messages
            .into_iter()
            .map(agent_message_to_planner_message),
    );
    if !untrusted_refs.is_empty() {
        let payload = serde_json::to_string(&untrusted_refs)
            .unwrap_or_else(|err| panic!("failed to serialize untrusted lcm refs: {err}"));
        messages.push(PlannerConversationMessage {
            role: PlannerConversationRole::User,
            kind: PlannerConversationMessageKind::RedactedInfo,
            content: format!("TRUSTED_LCM_UNTRUSTED_REFS\n{payload}"),
        });
    }
    messages
}

fn agent_message_to_planner_message(message: AgentMessage) -> PlannerConversationMessage {
    PlannerConversationMessage {
        role: if message.role == "assistant" {
            PlannerConversationRole::Assistant
        } else {
            PlannerConversationRole::User
        },
        kind: PlannerConversationMessageKind::FullText,
        content: agent_message_content_to_text(&message.content),
    }
}

fn agent_message_content_to_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let text_blocks = items
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                item.get("text")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect::<Vec<_>>();
        if !text_blocks.is_empty() {
            return text_blocks.join("\n");
        }
    }
    serde_json::to_string(content).unwrap_or_default()
}
