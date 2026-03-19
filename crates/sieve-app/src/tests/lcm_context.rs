use super::*;
use rusqlite::{params, Connection};
use sieve_types::PlannerConversationMessageKind;
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

struct CapturingPlannerModel {
    outputs: StdMutex<VecDeque<Result<PlannerTurnOutput, LlmError>>>,
    calls: AtomicU64,
    second_input: StdMutex<Option<PlannerTurnInput>>,
    config: LlmModelConfig,
}

impl CapturingPlannerModel {
    fn new(outputs: Vec<Result<PlannerTurnOutput, LlmError>>) -> Self {
        Self {
            outputs: StdMutex::new(VecDeque::from(outputs)),
            calls: AtomicU64::new(0),
            second_input: StdMutex::new(None),
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "planner-lcm-context-test".to_string(),
                api_base: None,
            },
        }
    }

    fn second_input(&self) -> PlannerTurnInput {
        self.second_input
            .lock()
            .expect("second planner input lock poisoned")
            .clone()
            .expect("captured second planner input")
    }
}

#[async_trait]
impl PlannerModel for CapturingPlannerModel {
    fn config(&self) -> &LlmModelConfig {
        &self.config
    }

    async fn plan_turn(&self, input: PlannerTurnInput) -> Result<PlannerTurnOutput, LlmError> {
        let call_index = self.calls.fetch_add(1, Ordering::Relaxed);
        if call_index == 1 {
            *self
                .second_input
                .lock()
                .expect("second planner input lock poisoned") = Some(input);
        }
        self.outputs
            .lock()
            .expect("planner output queue lock poisoned")
            .pop_front()
            .unwrap_or_else(|| {
                Err(LlmError::Backend(
                    "planner output queue exhausted for lcm context test".to_string(),
                ))
            })
    }
}

fn lcm_enabled_harness(
    planner: Arc<dyn PlannerModel>,
    response_messages: Vec<&str>,
) -> AppE2eHarness {
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
    ]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(
        response_messages
            .into_iter()
            .map(|message| {
                Ok(sieve_llm::ResponseTurnOutput {
                    message: message.to_string(),
                    referenced_ref_ids: BTreeSet::new(),
                    summarized_ref_ids: BTreeSet::new(),
                })
            })
            .collect(),
    ));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ],
        E2E_POLICY_BASE,
    );
    let mut lcm_config = LcmIntegrationConfig::from_sieve_home(&harness.root);
    lcm_config.enabled = true;
    let lcm = Arc::new(LcmIntegration::new(lcm_config).expect("initialize lcm integration"));
    harness.with_lcm(Some(lcm))
}

fn seed_stale_open_loop(db_path: &std::path::Path) {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("create codex db dir");
    }
    let conn = Connection::open(db_path).expect("open codex db");
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS open_loops (
            loop_id TEXT PRIMARY KEY,
            session_key TEXT NOT NULL,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            goal_summary TEXT NOT NULL,
            subject_names_json TEXT NOT NULL,
            target_paths_json TEXT NOT NULL,
            assistant_context TEXT NOT NULL,
            next_expected_user_act TEXT NOT NULL,
            ready_for_execution INTEGER NOT NULL,
            linked_codex_session_id TEXT,
            linked_codex_session_name TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );",
    )
    .expect("create open_loops table");
    conn.execute(
        "INSERT OR REPLACE INTO open_loops (
            loop_id, session_key, kind, status, goal_summary,
            subject_names_json, target_paths_json, assistant_context,
            next_expected_user_act, ready_for_execution,
            linked_codex_session_id, linked_codex_session_name,
            created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, ?11, ?12)",
        params![
            "open-loop-stale",
            "main",
            "proposal",
            "open",
            "Are there any emails I need to worry about",
            "[\"emails\",\"worry\"]",
            "[]",
            "I can help check, but I need access to your email source first.",
            "answer_questions",
            0i64,
            1_773_598_302_319i64,
            1_773_598_302_319i64,
        ],
    )
    .expect("insert stale open loop");
}

#[tokio::test]
async fn e2e_fake_lcm_auto_injects_trusted_memory_into_planner_conversation() {
    let planner = Arc::new(CapturingPlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
    ]));
    let harness = lcm_enabled_harness(
        planner.clone(),
        vec!["Thanks for sharing.", "You live in Livermore ca."],
    );

    harness
        .run_text_turn("Hi I live in Livermore ca")
        .await
        .expect("first memory turn should succeed");
    harness
        .run_text_turn("Where do I live?")
        .await
        .expect("follow-up turn should succeed");

    let second_input = planner.second_input();
    assert_eq!(second_input.user_message, "Where do I live?");
    assert!(
        second_input.conversation.iter().any(|message| {
            message.kind == PlannerConversationMessageKind::FullText
                && message.content.contains("Hi I live in Livermore ca")
        }),
        "trusted user memory should be injected into planner conversation"
    );
}

#[tokio::test]
async fn e2e_fake_lcm_keeps_untrusted_assistant_content_opaque_in_planner_conversation() {
    let planner = Arc::new(CapturingPlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
    ]));
    let harness = lcm_enabled_harness(
        planner.clone(),
        vec!["assistant secret raw string", "follow-up reply"],
    );

    harness
        .run_text_turn("remember this trusted fact")
        .await
        .expect("first memory turn should succeed");
    harness
        .run_text_turn("what do you remember?")
        .await
        .expect("follow-up turn should succeed");

    let second_input = planner.second_input();
    assert!(
        second_input
            .conversation
            .iter()
            .all(|message| !message.content.contains("assistant secret raw string")),
        "planner conversation must not include raw untrusted assistant content"
    );
    assert!(
        second_input.conversation.iter().any(|message| {
            message.kind == PlannerConversationMessageKind::RedactedInfo
                && message.content.contains("TRUSTED_LCM_UNTRUSTED_REFS")
        }),
        "planner should receive opaque untrusted refs instead of raw assistant text"
    );
}

#[tokio::test]
async fn e2e_fake_stale_open_loop_rows_are_not_injected_into_planner_conversation() {
    let planner = Arc::new(CapturingPlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: None,
            tool_calls: Vec::new(),
        }),
    ]));
    let harness = lcm_enabled_harness(planner.clone(), vec!["first", "second"]);
    seed_stale_open_loop(&harness.cfg.codex_store_path);

    harness
        .run_text_turn("Hi, I live in Livermore, CA")
        .await
        .expect("first turn should succeed");
    harness
        .run_text_turn("Any gmails that need my attention?")
        .await
        .expect("second turn should succeed");

    let second_input = planner.second_input();
    assert!(
        !second_input
            .conversation
            .iter()
            .any(|message| message.content.contains("TRUSTED_OPEN_LOOP_CONTEXT")),
        "stale open-loop rows should not be injected into planner conversation"
    );
}
