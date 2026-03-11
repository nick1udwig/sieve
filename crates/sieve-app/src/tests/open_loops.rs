use super::*;
use sieve_types::PlannerCodexSession;

const SIEVE_TUTOR_PROJECT_PROMPT: &str = "create a new project in ~/git/sieve-tutor

it is going to be a golang cli (cobra for parsing args). the purpose is to provide a learning-science-aware tool and long-term storage of learner state for an agent framework

specifically it should have subcommands that allow doing things like:
- storing a learners mastery of a specific topic (e.g., say learner is studying algebra; have broken down algebra into a directed graph of concepts required for learned, where direciton on graph indicates dependencies; provide lessons and then test comprehension on each of these concepts before marking as \"understood\"; use spaced repetition to solidify \"understood\" pieces of knowledge over time)
- a tool to scaffold a learner, given their state and a concept they are struggling on, to help them to understand that concept
- a tool to break down one or more texts or resources into conceptual dependency graphs

ask clarifying questions if you ahve any and then proceed";

const SIEVE_TUTOR_OPTIONS_TEXT: &str = "Before I start building, a few quick decisions will help me scaffold the right thing:

1. Storage format
- Do you want local-first storage with something simple like SQLite, or file-based JSON/YAML?
- My default would be SQLite.

2. CLI scope for v1
I’m thinking of an initial Cobra CLI with subcommands like:
- `learner set-mastery`
- `learner get-state`
- `review due`
- `review mark`
- `scaffold explain`
- `graph import-text`
- `graph show`
- `lesson recommend`

Does that match what you want for a first cut?

3. LLM / agent integration
- Should I design this as:
  - a standalone CLI only, or
  - a CLI plus a Go package intended to be called by an agent framework?
- My default would be both: clean internal packages plus CLI wrappers.

4. Graph extraction
- For “break down texts/resources into conceptual dependency graphs,” should v1 be:
  - manual/semi-structured import only, or
  - include an abstraction layer for AI-assisted extraction later?
- My default would be: implement manual/structured import now, define interfaces for future AI extraction.

5. Learner model
- Should mastery be a simple discrete state like `unknown -> learning -> understood`, or do you want a numeric confidence/mastery score plus spaced repetition scheduling?
- My default would be a numeric mastery/confidence model plus review scheduling metadata.

If you don’t answer, I’ll proceed with these defaults:
- SQLite
- Cobra CLI with the subcommands above
- CLI + reusable Go packages
- manual import + interfaces for future AI extraction
- numeric mastery + spaced repetition scheduling

Once you confirm or if you want me to proceed with defaults, I’ll create the project in `~/git/sieve-tutor`.";

struct OpenLoopAwarePlannerModel {
    outputs: StdMutex<VecDeque<Result<PlannerTurnOutput, LlmError>>>,
    calls: AtomicU64,
    second_input: StdMutex<Option<PlannerTurnInput>>,
    config: LlmModelConfig,
}

impl OpenLoopAwarePlannerModel {
    fn new(outputs: Vec<Result<PlannerTurnOutput, LlmError>>) -> Self {
        Self {
            outputs: StdMutex::new(VecDeque::from(outputs)),
            calls: AtomicU64::new(0),
            second_input: StdMutex::new(None),
            config: LlmModelConfig {
                provider: LlmProvider::OpenAi,
                model: "planner-open-loop-test".to_string(),
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
impl PlannerModel for OpenLoopAwarePlannerModel {
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
                    "planner output queue exhausted for open-loop test".to_string(),
                ))
            })
    }
}

#[tokio::test]
async fn e2e_fake_preference_turn_preserves_questions_and_tracks_open_loop() {
    let planner: Arc<dyn PlannerModel> =
        Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
            thoughts: Some(SIEVE_TUTOR_OPTIONS_TEXT.to_string()),
            tool_calls: Vec::new(),
        })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "I’ll proceed with the defaults and create the project.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner,
            guidance,
            response,
            summary,
        },
        vec!["codex_session".to_string()],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn(SIEVE_TUTOR_PROJECT_PROMPT)
        .await
        .expect("preference turn should succeed");

    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(assistant.len(), 1);
    assert_eq!(assistant[0], SIEVE_TUTOR_OPTIONS_TEXT);

    let store = OpenLoopStore::new(harness.cfg.codex_store_path.clone()).expect("open loop store");
    let loops = store
        .planner_open_loops("main", 8)
        .expect("planner open loops");
    assert_eq!(loops.len(), 1);
    assert!(loops[0].ready_for_execution);
    assert!(loops[0]
        .target_paths
        .iter()
        .any(|value: &String| value.contains("sieve-tutor")));
    assert!(loops[0]
        .subject_names
        .iter()
        .any(|value| value == "sieve-tutor"));
}

#[tokio::test]
async fn e2e_fake_go_ahead_binds_to_open_loop_before_saved_codex_session() {
    let planner = Arc::new(OpenLoopAwarePlannerModel::new(vec![
        Ok(PlannerTurnOutput {
            thoughts: Some(SIEVE_TUTOR_OPTIONS_TEXT.to_string()),
            tool_calls: Vec::new(),
        }),
        Ok(PlannerTurnOutput {
            thoughts: Some("start the new sieve-tutor codex session".to_string()),
            tool_calls: vec![PlannerToolCall {
                tool_name: "codex_session".to_string(),
                args: BTreeMap::from([
                    (
                        "instruction".to_string(),
                        serde_json::json!(
                            "start the new project from the approved sieve-tutor plan"
                        ),
                    ),
                    ("sandbox".to_string(), serde_json::json!("workspace_write")),
                    ("cwd".to_string(), serde_json::json!("~/git/sieve-tutor")),
                    (
                        "writable_roots".to_string(),
                        serde_json::json!(["~/git/sieve-tutor"]),
                    ),
                ]),
            }],
        }),
    ]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![
        Ok(guidance_output(
            PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint,
        )),
        Ok(guidance_output(PlannerGuidanceSignal::FinalAnswerReady)),
    ]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "Started a new Codex session for `sieve-tutor`.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let codex = Arc::new(TestCodexTool::new(vec![Ok(
        sieve_runtime::CodexSessionToolResult {
            result: sieve_types::CodexTurnResult {
                session_id: Some("codex-session-sieve-tutor".to_string()),
                session_name: "sieve-tutor".to_string(),
                status: sieve_types::CodexTurnStatus::NeedsFollowup,
                summary: "scaffolded the new sieve-tutor repo".to_string(),
                user_visible: Some("Created the repo and first files.".to_string()),
                turn_id: Some("turn-1".to_string()),
                thread_id: Some("thread-1".to_string()),
            },
        },
    )]));
    codex.set_planner_sessions(vec![PlannerCodexSession {
        session_id: "codex-session-modex".to_string(),
        session_name: "you-are-starting".to_string(),
        cwd: "/root/git/modex".to_string(),
        sandbox: sieve_types::CodexSandboxMode::WorkspaceWrite,
        updated_at_utc: "2026-03-10T17:36:42Z".to_string(),
        status: "completed".to_string(),
        task_summary: "build modex".to_string(),
        last_result_summary: Some("a passing npm run build".to_string()),
    }]);
    let harness = AppE2eHarness::new_with_codex(
        E2eModelMode::Fake {
            planner: planner.clone(),
            guidance,
            response,
            summary,
        },
        vec!["codex_session".to_string()],
        E2E_POLICY_BASE,
        Some(codex.clone()),
    );

    harness
        .run_text_turn(SIEVE_TUTOR_PROJECT_PROMPT)
        .await
        .expect("planning turn should succeed");
    harness
        .run_text_turn("ok go ahead. use codex")
        .await
        .expect("go-ahead turn should succeed");

    let second_input = planner.second_input();
    assert!(
        second_input
            .user_message
            .contains("TRUSTED_OPEN_LOOP_CONTEXT"),
        "planner should receive trusted open-loop context on go-ahead followup"
    );
    assert!(
        second_input.user_message.contains("sieve-tutor"),
        "planner context should preserve the new project target"
    );
    assert!(
        second_input
            .codex_sessions
            .iter()
            .any(|session| session.cwd == "/root/git/modex"
                || session.session_name == "you-are-starting"),
        "test should still expose the prior modex session so binding can prefer the open loop"
    );

    let requests = codex.session_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].session_id, None);
    assert!(requests[0]
        .cwd
        .as_deref()
        .unwrap_or_default()
        .contains("sieve-tutor"));
}

#[tokio::test]
async fn e2e_fake_status_query_uses_open_loop_before_planner() {
    let planner = Arc::new(QueuedPlannerModel::new(vec![Ok(PlannerTurnOutput {
        thoughts: Some(SIEVE_TUTOR_OPTIONS_TEXT.to_string()),
        tool_calls: Vec::new(),
    })]));
    let guidance: Arc<dyn GuidanceModel> = Arc::new(QueuedGuidanceModel::new(vec![Ok(
        guidance_output(PlannerGuidanceSignal::ContinueNeedPreferenceOrConstraint),
    )]));
    let response: Arc<dyn ResponseModel> = Arc::new(QueuedResponseModel::new(vec![Ok(
        sieve_llm::ResponseTurnOutput {
            message: "I’ll proceed with the defaults and create the project.".to_string(),
            referenced_ref_ids: BTreeSet::new(),
            summarized_ref_ids: BTreeSet::new(),
        },
    )]));
    let summary: Arc<dyn SummaryModel> = Arc::new(EchoSummaryModel);
    let harness = AppE2eHarness::new(
        E2eModelMode::Fake {
            planner: planner.clone(),
            guidance,
            response,
            summary,
        },
        vec!["codex_session".to_string()],
        E2E_POLICY_BASE,
    );

    harness
        .run_text_turn(SIEVE_TUTOR_PROJECT_PROMPT)
        .await
        .expect("planning turn should succeed");
    harness
        .run_text_turn("how is sieve-tutor work going?")
        .await
        .expect("status turn should succeed");

    let assistant = assistant_messages(&harness.runtime_events());
    assert_eq!(assistant.len(), 2);
    assert!(
        assistant[1].contains("haven’t started") || assistant[1].contains("haven't started"),
        "status reply should say the work has not started yet"
    );
    assert!(
        assistant[1].contains("go-ahead") || assistant[1].contains("answers"),
        "status reply should come from stored open-loop state"
    );
    assert_eq!(planner.call_count(), 1);
}
