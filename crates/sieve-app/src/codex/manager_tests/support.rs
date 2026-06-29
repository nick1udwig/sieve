#[derive(Default)]
struct RecordingEventLog {
    events: Mutex<Vec<RuntimeEvent>>,
}

#[async_trait]
impl RuntimeEventLog for RecordingEventLog {
    async fn append(&self, event: RuntimeEvent) -> Result<(), sieve_runtime::EventLogError> {
        self.events.lock().expect("event log lock").push(event);
        Ok(())
    }
}

fn test_manager(
    root: &Path,
    script: &Path,
    approval_bus: Arc<InProcessApprovalBus>,
    event_log: Arc<RecordingEventLog>,
) -> CodexManager {
    CodexManager {
        config: CodexManagerConfig {
            program: Some(script.to_string_lossy().to_string()),
            ws_url: None,
            connect_timeout_ms: DEFAULT_CODEX_CONNECT_TIMEOUT_MS,
            model: Some("mock-model".to_string()),
            turn_timeout_ms: 5_000,
            request_timeout_ms: 5_000,
        },
        store: CodexSessionStore::new(root.join("state/codex.db")).expect("create codex store"),
        approval_bus,
        event_log,
        active_sessions: Mutex::new(HashMap::new()),
    }
}

async fn wait_for_active_planner_session(manager: &CodexManager) -> PlannerCodexSession {
    for _ in 0..200 {
        if let Ok(sessions) = manager.planner_sessions().await {
            for session in sessions {
                let is_active = manager
                    .active_sessions
                    .lock()
                    .expect("active session lock")
                    .contains_key(&session.session_id);
                if is_active {
                    return session;
                }
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("planner session was not active");
}

async fn wait_for_published_approval(bus: &InProcessApprovalBus) -> ApprovalRequestedEvent {
    for _ in 0..200 {
        if let Ok(events) = bus.published_events() {
            if let Some(event) = events.last() {
                return event.clone();
            }
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("approval was not published");
}

async fn auto_approve_codex_requests(bus: Arc<InProcessApprovalBus>) {
    let mut resolved = HashSet::new();
    loop {
        if let Ok(events) = bus.published_events() {
            for event in events {
                if !resolved.insert(event.request_id.clone()) {
                    continue;
                }
                let action = match event.prompt_kind {
                    ApprovalPromptKind::Command => ApprovalAction::ApproveOnce,
                    ApprovalPromptKind::FileChange => ApprovalAction::ApproveAlways,
                };
                let _ = bus.resolve(ApprovalResolvedEvent {
                    schema_version: 1,
                    request_id: event.request_id.clone(),
                    run_id: event.run_id.clone(),
                    action,
                    created_at_ms: now_ms(),
                });
            }
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn write_mock_server(root: &Path, body: &str) -> PathBuf {
    let path = root.join(format!("mock-codex-server-{}.py", Uuid::new_v4()));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create mock server dir");
    }
    let mut file = fs::File::create(&path).expect("create mock server");
    file.write_all(body.as_bytes()).expect("write mock server");
    let mut permissions = file.metadata().expect("mock server metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod mock server");
    path
}

fn unique_test_root(prefix: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).expect("create test root");
    root
}
