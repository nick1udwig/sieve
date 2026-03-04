#![forbid(unsafe_code)]

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use sieve_lcm::db::config::{resolve_lcm_config, LcmConfig};
use sieve_lcm::engine::{AssembleInput, IngestInput, LcmContextEngine, LcmContextEngineApi};
use sieve_lcm::tools::common::ToolResult;
use sieve_lcm::tools::lcm_describe_tool::create_lcm_describe_tool;
use sieve_lcm::tools::lcm_expand_query_tool::create_lcm_expand_query_tool;
use sieve_lcm::tools::lcm_expand_tool::create_lcm_expand_tool;
use sieve_lcm::tools::lcm_grep_tool::create_lcm_grep_tool;
use sieve_lcm::types::{
    AgentMessage, CompletionContentBlock, CompletionRequest, CompletionResult, GatewayCallRequest,
    LcmDependencies, LcmLogger, ModelRef,
};
use sieve_types::RunId;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use uuid::Uuid;

const OPENAI_DEFAULT_API_BASE: &str = "https://api.openai.com";
const SUBAGENT_MAX_STEPS: usize = 8;
const DEFAULT_SUBAGENT_MAX_COMPLETION_TOKENS: i64 = 1200;

#[derive(Clone)]
pub struct LcmIntegrationConfig {
    pub enabled: bool,
    pub enable_untrusted_refs: bool,
    pub global_session_id: String,
    pub trusted_db_path: PathBuf,
    pub untrusted_db_path: PathBuf,
    pub planner_context_token_budget: i64,
    pub untrusted_ref_token_budget: i64,
    pub summary_model: String,
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
        let planner_context_token_budget =
            parse_i64_env("SIEVE_LCM_PLANNER_CONTEXT_TOKENS", 12000).max(512);
        let untrusted_ref_token_budget =
            parse_i64_env("SIEVE_LCM_UNTRUSTED_REF_TOKENS", 12000).max(512);
        let summary_model = env::var("SIEVE_LCM_SUMMARY_MODEL")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .or_else(|| {
                env::var("SIEVE_QUARANTINE_MODEL")
                    .ok()
                    .map(|raw| raw.trim().to_string())
                    .filter(|raw| !raw.is_empty())
            })
            .or_else(|| {
                env::var("SIEVE_PLANNER_MODEL")
                    .ok()
                    .map(|raw| raw.trim().to_string())
                    .filter(|raw| !raw.is_empty())
            })
            .unwrap_or_else(|| "gpt-5.2".to_string());

        Self {
            enabled: parse_bool_env("SIEVE_LCM_ENABLED", true),
            enable_untrusted_refs: parse_bool_env("SIEVE_LCM_ENABLE_UNTRUSTED_REFS", true),
            global_session_id,
            trusted_db_path,
            untrusted_db_path,
            planner_context_token_budget,
            untrusted_ref_token_budget,
            summary_model,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LcmMemoryRef {
    pub ref_id: String,
    pub path: PathBuf,
    pub byte_count: u64,
    pub line_count: u64,
}

#[derive(Clone)]
pub struct LcmIntegration {
    trusted: Arc<LcmLane>,
    untrusted: Arc<LcmLane>,
    enable_untrusted_refs: bool,
    planner_context_token_budget: i64,
    untrusted_ref_token_budget: i64,
}

impl LcmIntegration {
    pub fn new(config: LcmIntegrationConfig) -> Result<Self, String> {
        if !config.enabled {
            return Err("lcm integration disabled".to_string());
        }

        let trusted = LcmLane::new(
            "trusted",
            &config.global_session_id,
            &config.trusted_db_path,
            &config.summary_model,
        )?;
        let untrusted = LcmLane::new(
            "untrusted",
            &config.global_session_id,
            &config.untrusted_db_path,
            &config.summary_model,
        )?;

        Ok(Self {
            trusted: Arc::new(trusted),
            untrusted: Arc::new(untrusted),
            enable_untrusted_refs: config.enable_untrusted_refs,
            planner_context_token_budget: config.planner_context_token_budget,
            untrusted_ref_token_budget: config.untrusted_ref_token_budget,
        })
    }

    pub async fn ingest_user_message(&self, message: &str) -> Result<(), String> {
        self.trusted.ingest_text("user", message).await?;
        self.untrusted.ingest_text("user", message).await
    }

    pub async fn ingest_assistant_message(&self, message: &str) -> Result<(), String> {
        self.untrusted.ingest_text("assistant", message).await
    }

    pub async fn trusted_planner_context(
        &self,
        current_user_message: &str,
    ) -> Result<Option<String>, String> {
        let assembled = self
            .trusted
            .assemble_context_text(current_user_message, self.planner_context_token_budget)
            .await?;
        let normalized = assembled.trim().to_string();
        if normalized.is_empty() {
            Ok(None)
        } else {
            Ok(Some(normalized))
        }
    }

    pub async fn build_untrusted_ref_for_qllm(
        &self,
        run_id: &RunId,
        sieve_home: &Path,
        user_query: &str,
    ) -> Result<Option<LcmMemoryRef>, String> {
        if !self.enable_untrusted_refs {
            return Ok(None);
        }
        let prompt = user_query.trim();
        if prompt.is_empty() {
            return Ok(None);
        }

        // Always try delegated expand-query first; fallback to assembled context text.
        let delegated = self
            .untrusted
            .delegated_expand_query(prompt, self.untrusted_ref_token_budget)
            .await;

        let payload = match delegated {
            Ok(tool_result) => tool_result
                .content
                .first()
                .map(|block| block.text.clone())
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| {
                    serde_json::to_string_pretty(&tool_result.details)
                        .unwrap_or_else(|_| "{}".to_string())
                }),
            Err(err) => {
                let fallback = self
                    .untrusted
                    .assemble_context_text(prompt, self.untrusted_ref_token_budget)
                    .await
                    .unwrap_or_default();
                if fallback.trim().is_empty() {
                    format!("{{\n  \"error\": \"{}\"\n}}", err.replace('"', "\\\""))
                } else {
                    fallback
                }
            }
        };

        let mem_ref = persist_memory_ref_artifact(
            run_id,
            sieve_home,
            "untrusted-context",
            payload.as_bytes(),
        )
        .await?;

        Ok(Some(mem_ref))
    }
}

#[derive(Clone)]
struct LcmLane {
    engine: Arc<LcmContextEngine>,
    deps: Arc<AppLcmDeps>,
    global_session_id: String,
}

impl LcmLane {
    fn new(
        lane_name: &str,
        global_session_id: &str,
        db_path: &Path,
        summary_model: &str,
    ) -> Result<Self, String> {
        let mut config = resolve_lcm_config();
        config.enabled = true;
        config.database_path = db_path.to_string_lossy().to_string();

        let deps = Arc::new(AppLcmDeps::new(
            lane_name.to_string(),
            global_session_id.to_string(),
            config,
            summary_model.to_string(),
        )?);
        deps.set_self_ref(Arc::downgrade(&deps));

        let engine = Arc::new(
            LcmContextEngine::from_dependencies(deps.clone())
                .map_err(|err| format!("failed to initialize {lane_name} lcm engine: {err}"))?,
        );

        deps.set_engine(engine.clone());

        Ok(Self {
            engine,
            deps,
            global_session_id: global_session_id.to_string(),
        })
    }

    async fn ingest_text(&self, role: &str, content: &str) -> Result<(), String> {
        if content.trim().is_empty() {
            return Ok(());
        }

        self.engine
            .ingest(IngestInput {
                session_id: self.global_session_id.clone(),
                message: AgentMessage::new_text(role, content),
                is_heartbeat: Some(false),
            })
            .await
            .map_err(|err| format!("lcm ingest failed: {err}"))?;
        Ok(())
    }

    async fn assemble_context_text(
        &self,
        current_user_message: &str,
        token_budget: i64,
    ) -> Result<String, String> {
        let assembled = self
            .engine
            .assemble(AssembleInput {
                session_id: self.global_session_id.clone(),
                messages: vec![AgentMessage::new_text("user", current_user_message)],
                token_budget: Some(token_budget.max(512)),
            })
            .await
            .map_err(|err| format!("lcm assemble failed: {err}"))?;

        Ok(flatten_agent_messages(&assembled.messages))
    }

    async fn delegated_expand_query(
        &self,
        prompt: &str,
        token_cap: i64,
    ) -> Result<ToolResult, String> {
        let deps: Arc<dyn LcmDependencies> = self.deps.clone();
        let lcm: Arc<dyn LcmContextEngineApi> = self.engine.clone();
        let session_key = "agent:main:main".to_string();
        let tool = create_lcm_expand_query_tool(
            deps,
            lcm,
            Some(self.global_session_id.clone()),
            Some(session_key.clone()),
            Some(session_key),
        );
        let params = json!({
            "prompt": prompt,
            "query": prompt,
            "maxTokens": DEFAULT_SUBAGENT_MAX_COMPLETION_TOKENS,
            "tokenCap": token_cap.max(512),
        });
        tool.execute("lcm-expand-query-root", params)
            .await
            .map_err(|err| format!("lcm expand_query failed: {err}"))
    }
}

#[derive(Debug, Clone)]
struct GatewayRunState {
    status: String,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct GatewayState {
    runs: HashMap<String, GatewayRunState>,
    sessions: HashMap<String, Vec<Value>>,
}

#[derive(Clone)]
struct AppLcmLogger {
    lane_name: String,
}

impl LcmLogger for AppLcmLogger {
    fn info(&self, msg: &str) {
        eprintln!("[lcm:{}][info] {}", self.lane_name, msg);
    }

    fn warn(&self, msg: &str) {
        eprintln!("[lcm:{}][warn] {}", self.lane_name, msg);
    }

    fn error(&self, msg: &str) {
        eprintln!("[lcm:{}][error] {}", self.lane_name, msg);
    }

    fn debug(&self, msg: &str) {
        eprintln!("[lcm:{}][debug] {}", self.lane_name, msg);
    }
}

struct AppLcmDeps {
    global_session_id: String,
    config: LcmConfig,
    model: String,
    api_key: String,
    api_base: String,
    http: Client,
    logger: AppLcmLogger,
    self_ref: Mutex<Weak<AppLcmDeps>>,
    engine: Mutex<Option<Arc<LcmContextEngine>>>,
    gateway_state: Mutex<GatewayState>,
}

impl AppLcmDeps {
    fn new(
        lane_name: String,
        global_session_id: String,
        config: LcmConfig,
        model: String,
    ) -> Result<Self, String> {
        let api_key = load_openai_api_key()?;
        let api_base = env::var("SIEVE_PLANNER_API_BASE")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| OPENAI_DEFAULT_API_BASE.to_string());
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(40))
            .build()
            .map_err(|err| format!("failed to build lcm http client: {err}"))?;

        Ok(Self {
            global_session_id,
            config,
            model,
            api_key,
            api_base,
            http,
            logger: AppLcmLogger { lane_name },
            self_ref: Mutex::new(Weak::new()),
            engine: Mutex::new(None),
            gateway_state: Mutex::new(GatewayState::default()),
        })
    }

    fn set_self_ref(&self, weak: Weak<AppLcmDeps>) {
        if let Ok(mut guard) = self.self_ref.lock() {
            *guard = weak;
        }
    }

    fn set_engine(&self, engine: Arc<LcmContextEngine>) {
        if let Ok(mut guard) = self.engine.lock() {
            *guard = Some(engine);
        }
    }

    fn self_arc(&self) -> Result<Arc<AppLcmDeps>, String> {
        self.self_ref
            .lock()
            .map_err(|_| "lcm self_ref lock poisoned".to_string())?
            .upgrade()
            .ok_or_else(|| "lcm self arc unavailable".to_string())
    }

    fn engine_arc(&self) -> Result<Arc<LcmContextEngine>, String> {
        self.engine
            .lock()
            .map_err(|_| "lcm engine lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "lcm engine unavailable".to_string())
    }

    async fn execute_subagent(
        self: Arc<Self>,
        message: String,
        session_key: String,
        extra_system_prompt: Option<String>,
    ) -> Result<Vec<Value>, String> {
        let system_prompt = format!(
            "You are an LCM delegated retrieval sub-agent. Use only lcm tools (lcm_describe, lcm_expand, lcm_grep). Never use bash or external tools. Return concise output. {}",
            extra_system_prompt.unwrap_or_default()
        );

        let mut transcript = vec![
            json!({"role":"system","content":system_prompt.clone()}),
            json!({"role":"user","content":message.clone()}),
        ];

        let mut messages = vec![
            json!({"role":"system","content":system_prompt}),
            json!({"role":"user","content":message}),
        ];

        let tools = delegated_tool_definitions();
        for _ in 0..SUBAGENT_MAX_STEPS {
            let response = self
                .openai_chat_completion(messages.clone(), &tools)
                .await
                .map_err(|err| format!("delegated chat completion failed: {err}"))?;
            let message_obj = response
                .pointer("/choices/0/message")
                .and_then(Value::as_object)
                .ok_or_else(|| "delegated completion missing choices[0].message".to_string())?
                .clone();

            let assistant_content = message_obj
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            let tool_calls = message_obj
                .get("tool_calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            if !tool_calls.is_empty() {
                messages.push(json!({
                    "role": "assistant",
                    "content": assistant_content,
                    "tool_calls": tool_calls,
                }));

                for tool_call in &tool_calls {
                    let tool_id = tool_call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let tool_name = tool_call
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let args_raw = tool_call
                        .pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}")
                        .to_string();
                    let args_json =
                        serde_json::from_str::<Value>(&args_raw).unwrap_or_else(|_| json!({}));

                    let tool_text = self
                        .execute_delegated_tool(&session_key, &tool_name, &tool_id, args_json)
                        .await
                        .unwrap_or_else(|err| {
                            format!("{{\"error\":\"{}\"}}", err.replace('"', "\\\""))
                        });

                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_id,
                        "content": tool_text,
                    }));
                    transcript.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_id,
                        "tool_name": tool_name,
                        "content": tool_text,
                    }));
                }

                continue;
            }

            transcript.push(json!({
                "role": "assistant",
                "content": assistant_content,
            }));
            return Ok(transcript);
        }

        transcript.push(json!({
            "role": "assistant",
            "content": "{\"error\":\"delegated sub-agent exhausted tool loop\"}",
        }));
        Ok(transcript)
    }

    async fn execute_delegated_tool(
        &self,
        session_key: &str,
        tool_name: &str,
        tool_call_id: &str,
        args: Value,
    ) -> Result<String, String> {
        let deps: Arc<dyn LcmDependencies> = self.self_arc()?;
        let lcm: Arc<dyn LcmContextEngineApi> = self.engine_arc()?;

        let tool_result = match tool_name {
            "lcm_describe" => {
                create_lcm_describe_tool(
                    deps,
                    lcm,
                    Some(self.global_session_id.clone()),
                    Some(session_key.to_string()),
                )
                .execute(tool_call_id, args)
                .await
            }
            "lcm_expand" => {
                create_lcm_expand_tool(
                    deps,
                    lcm,
                    Some(self.global_session_id.clone()),
                    Some(session_key.to_string()),
                )
                .execute(tool_call_id, args)
                .await
            }
            "lcm_grep" => {
                create_lcm_grep_tool(
                    deps,
                    lcm,
                    Some(self.global_session_id.clone()),
                    Some(session_key.to_string()),
                )
                .execute(tool_call_id, args)
                .await
            }
            "lcm_expand_query" => {
                return Err("lcm_expand_query is disallowed in delegated sessions".to_string())
            }
            other => return Err(format!("unknown delegated tool `{other}`")),
        }
        .map_err(|err| format!("delegated tool `{tool_name}` failed: {err}"))?;

        Ok(tool_result
            .content
            .first()
            .map(|block| block.text.clone())
            .unwrap_or_else(|| {
                serde_json::to_string_pretty(&tool_result.details)
                    .unwrap_or_else(|_| "{}".to_string())
            }))
    }

    async fn openai_chat_completion(
        &self,
        messages: Vec<Value>,
        tools: &[Value],
    ) -> Result<Value, String> {
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.api_base.trim_end_matches('/')
        );

        let payload = json!({
            "model": self.model.clone(),
            "temperature": 0,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        });

        let response = self
            .http
            .post(&endpoint)
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|err| format!("request transport failed: {err}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|err| format!("failed to read body: {err}"))?;

        if !status.is_success() {
            return Err(format!(
                "status {} from OpenAI: {}",
                status.as_u16(),
                truncate_text(&body, 640)
            ));
        }

        serde_json::from_str::<Value>(&body)
            .map_err(|err| format!("failed to decode json body: {err}"))
    }

    fn resolve_api_key_value(&self, override_key: Option<&str>) -> String {
        override_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| self.api_key.clone())
    }

    async fn gateway_agent_call(&self, params: Value) -> Result<Value, String> {
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "gateway method `agent` requires non-empty `message`".to_string())?
            .to_string();
        let session_key = params
            .get("sessionKey")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "gateway method `agent` requires non-empty `sessionKey`".to_string())?
            .to_string();
        let extra_system_prompt = params
            .get("extraSystemPrompt")
            .and_then(Value::as_str)
            .map(|value| value.to_string());

        let run_id = Uuid::new_v4().to_string();
        let self_arc = self.self_arc()?;
        let task_message = message.clone();
        let task_session_key = session_key.clone();
        let task_extra_system_prompt = extra_system_prompt.clone();
        let result = tokio::task::spawn_blocking(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| format!("failed to build delegated runtime: {err}"))?;
            runtime.block_on(self_arc.execute_subagent(
                task_message,
                task_session_key,
                task_extra_system_prompt,
            ))
        })
        .await
        .map_err(|err| format!("delegated sub-agent join failed: {err}"))?;

        let mut state = self
            .gateway_state
            .lock()
            .map_err(|_| "gateway state lock poisoned".to_string())?;
        match result {
            Ok(transcript) => {
                state.sessions.insert(session_key, transcript);
                state.runs.insert(
                    run_id.clone(),
                    GatewayRunState {
                        status: "ok".to_string(),
                        error: None,
                    },
                );
            }
            Err(error) => {
                state.runs.insert(
                    run_id.clone(),
                    GatewayRunState {
                        status: "error".to_string(),
                        error: Some(error),
                    },
                );
            }
        }

        Ok(json!({ "runId": run_id }))
    }

    fn gateway_wait_call(&self, params: Value) -> Result<Value, String> {
        let run_id = params
            .get("runId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "gateway method `agent.wait` requires non-empty `runId`".to_string())?;

        let state = self
            .gateway_state
            .lock()
            .map_err(|_| "gateway state lock poisoned".to_string())?;
        let Some(run_state) = state.runs.get(run_id) else {
            return Ok(json!({
                "status": "error",
                "error": format!("unknown runId `{run_id}`"),
            }));
        };

        Ok(json!({
            "status": run_state.status.clone(),
            "error": run_state.error.clone(),
        }))
    }

    fn gateway_sessions_get_call(&self, params: Value) -> Result<Value, String> {
        let key = params
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "gateway method `sessions.get` requires non-empty `key`".to_string())?;

        let limit = params
            .get("limit")
            .and_then(Value::as_i64)
            .unwrap_or(80)
            .max(1) as usize;

        let state = self
            .gateway_state
            .lock()
            .map_err(|_| "gateway state lock poisoned".to_string())?;
        let mut messages = state.sessions.get(key).cloned().unwrap_or_default();
        if messages.len() > limit {
            let start = messages.len().saturating_sub(limit);
            messages = messages.split_off(start);
        }

        Ok(json!({ "messages": messages }))
    }

    fn gateway_sessions_delete_call(&self, params: Value) -> Result<Value, String> {
        let key = params
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "gateway method `sessions.delete` requires non-empty `key`".to_string()
            })?;

        let mut state = self
            .gateway_state
            .lock()
            .map_err(|_| "gateway state lock poisoned".to_string())?;
        let removed = state.sessions.remove(key).is_some();

        Ok(json!({ "ok": removed }))
    }
}

#[async_trait]
impl LcmDependencies for AppLcmDeps {
    fn config(&self) -> &LcmConfig {
        &self.config
    }

    async fn complete(&self, request: CompletionRequest) -> anyhow::Result<CompletionResult> {
        let endpoint = format!(
            "{}/v1/chat/completions",
            self.api_base.trim_end_matches('/')
        );

        let api_key = self.resolve_api_key_value(request.api_key.as_deref());
        let mut messages = Vec::new();
        if let Some(system) = request
            .system
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            messages.push(json!({
                "role": "system",
                "content": system,
            }));
        }

        for message in request.messages {
            let content = if let Some(content) = message.content.as_str() {
                content.to_string()
            } else {
                serde_json::to_string(&message.content).unwrap_or_default()
            };
            messages.push(json!({
                "role": message.role,
                "content": content,
            }));
        }

        let payload = json!({
            "model": if request.model.trim().is_empty() {
                self.model.clone()
            } else {
                request.model.clone()
            },
            "messages": messages,
            "max_completion_tokens": request.max_tokens.max(64),
            "temperature": request.temperature.unwrap_or(0.2),
        });

        let response = self
            .http
            .post(&endpoint)
            .bearer_auth(api_key)
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .context("lcm complete request failed")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("lcm complete body read failed")?;
        if !status.is_success() {
            return Err(anyhow!(
                "lcm complete status {}: {}",
                status.as_u16(),
                truncate_text(&body, 640)
            ));
        }

        let response_json = serde_json::from_str::<Value>(&body)
            .map_err(|err| anyhow!("lcm complete decode failed: {err}"))?;

        let text = response_json
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(CompletionResult {
            content: vec![CompletionContentBlock {
                r#type: "text".to_string(),
                text: Some(text),
                extra: HashMap::new(),
            }],
            extra: HashMap::new(),
        })
    }

    async fn call_gateway(&self, request: GatewayCallRequest) -> anyhow::Result<Value> {
        let params = request.params.unwrap_or_else(|| json!({}));
        let out = match request.method.as_str() {
            "agent" => self.gateway_agent_call(params).await,
            "agent.wait" => self.gateway_wait_call(params),
            "sessions.get" => self.gateway_sessions_get_call(params),
            "sessions.delete" => self.gateway_sessions_delete_call(params),
            other => Err(format!("unsupported gateway method `{other}`")),
        }
        .map_err(|err| anyhow!("gateway call failed: {err}"))?;

        Ok(out)
    }

    fn resolve_model(
        &self,
        model_ref: Option<&str>,
        provider_hint: Option<&str>,
    ) -> anyhow::Result<ModelRef> {
        let model = model_ref
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(self.model.as_str())
            .to_string();
        let provider = provider_hint
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("openai")
            .to_ascii_lowercase();
        Ok(ModelRef { provider, model })
    }

    fn get_api_key(&self, provider: &str, _model: &str) -> Option<String> {
        if provider.trim().eq_ignore_ascii_case("openai") {
            Some(self.api_key.clone())
        } else {
            None
        }
    }

    fn require_api_key(&self, provider: &str, _model: &str) -> anyhow::Result<String> {
        if provider.trim().eq_ignore_ascii_case("openai") {
            Ok(self.api_key.clone())
        } else {
            Err(anyhow!("unsupported provider `{provider}`"))
        }
    }

    fn parse_agent_session_key(&self, session_key: &str) -> Option<(String, String)> {
        parse_agent_session_key(session_key)
    }

    fn is_subagent_session_key(&self, session_key: &str) -> bool {
        session_key.contains(":subagent:")
    }

    fn normalize_agent_id(&self, id: Option<&str>) -> String {
        id.map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("main")
            .to_string()
    }

    fn build_subagent_system_prompt(
        &self,
        _depth: i32,
        _max_depth: i32,
        task_summary: Option<&str>,
    ) -> String {
        task_summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("Subagent task: {value}"))
            .unwrap_or_else(|| "Subagent task".to_string())
    }

    fn read_latest_assistant_reply(&self, messages: &[Value]) -> Option<String> {
        for message in messages.iter().rev() {
            let Some(role) = message.get("role").and_then(Value::as_str) else {
                continue;
            };
            if role != "assistant" {
                continue;
            }
            if let Some(content) = message.get("content").and_then(Value::as_str) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        None
    }

    fn resolve_agent_dir(&self) -> String {
        env::var("SIEVE_HOME")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| ".sieve".to_string())
    }

    async fn resolve_session_id_from_session_key(
        &self,
        session_key: &str,
    ) -> anyhow::Result<Option<String>> {
        let trimmed = session_key.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.global_session_id.clone()))
    }

    fn agent_lane_subagent(&self) -> &str {
        "subagent"
    }

    fn logger(&self) -> &dyn LcmLogger {
        &self.logger
    }
}

fn delegated_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "lcm_describe",
                "description": "Describe one summary/file node from LCM by id.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "conversationId": {"type": "integer"},
                        "allConversations": {"type": "boolean"},
                        "tokenCap": {"type": "integer"}
                    },
                    "required": ["id"],
                    "additionalProperties": true
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "lcm_expand",
                "description": "Expand summaries in delegated sub-agent sessions.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "summaryIds": {"type": "array", "items": {"type": "string"}},
                        "query": {"type": "string"},
                        "conversationId": {"type": "integer"},
                        "maxDepth": {"type": "integer"},
                        "tokenCap": {"type": "integer"},
                        "includeMessages": {"type": "boolean"}
                    },
                    "additionalProperties": true
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "lcm_grep",
                "description": "Search summaries/messages in LCM.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "mode": {"type": "string"},
                        "scope": {"type": "string"},
                        "conversationId": {"type": "integer"},
                        "allConversations": {"type": "boolean"},
                        "limit": {"type": "integer"},
                        "since": {"type": "string"},
                        "before": {"type": "string"}
                    },
                    "required": ["pattern"],
                    "additionalProperties": true
                }
            }
        }),
    ]
}

fn parse_agent_session_key(session_key: &str) -> Option<(String, String)> {
    let trimmed = session_key.trim();
    if !trimmed.starts_with("agent:") {
        return None;
    }
    let parts = trimmed.split(':').collect::<Vec<&str>>();
    if parts.len() < 3 {
        return None;
    }
    Some((
        parts.get(1).copied().unwrap_or("main").to_string(),
        parts[2..].join(":"),
    ))
}

fn flatten_agent_messages(messages: &[AgentMessage]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        let content = flatten_agent_message_content(&message.content);
        if content.trim().is_empty() {
            continue;
        }
        lines.push(format!("[{}] {}", message.role, content));
    }
    lines.join("\n")
}

fn flatten_agent_message_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let mut parts = Vec::new();
        for item in items {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                if !text.trim().is_empty() {
                    parts.push(text.to_string());
                }
            } else if let Some(value) = item.as_str() {
                if !value.trim().is_empty() {
                    parts.push(value.to_string());
                }
            }
        }
        if !parts.is_empty() {
            return parts.join("\n");
        }
    }
    serde_json::to_string(content).unwrap_or_default()
}

async fn persist_memory_ref_artifact(
    run_id: &RunId,
    sieve_home: &Path,
    stem: &str,
    bytes: &[u8],
) -> Result<LcmMemoryRef, String> {
    let run_dir = sieve_home.join("artifacts").join(&run_id.0);
    tokio::fs::create_dir_all(&run_dir)
        .await
        .map_err(|err| format!("create lcm artifact dir failed: {err}"))?;

    let ref_id = format!("lcm-{}-{}", stem, Uuid::new_v4().simple());
    let path = run_dir.join(format!("{ref_id}.log"));
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|err| format!("write lcm artifact failed: {err}"))?;

    Ok(LcmMemoryRef {
        ref_id,
        path,
        byte_count: bytes.len() as u64,
        line_count: count_newlines(bytes),
    })
}

fn count_newlines(bytes: &[u8]) -> u64 {
    bytes.iter().filter(|byte| **byte == b'\n').count() as u64
}

fn truncate_text(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}...[truncated]", &value[..max])
    }
}

fn parse_bool_env(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(raw) => match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn parse_i64_env(key: &str, default: i64) -> i64 {
    env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .unwrap_or(default)
}

fn load_openai_api_key() -> Result<String, String> {
    let planner_scoped = env::var("SIEVE_PLANNER_OPENAI_API_KEY")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty());
    let fallback = env::var("OPENAI_API_KEY")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty());
    planner_scoped.or(fallback).ok_or_else(|| {
        "missing OPENAI_API_KEY (or SIEVE_PLANNER_OPENAI_API_KEY) for lcm".to_string()
    })
}
