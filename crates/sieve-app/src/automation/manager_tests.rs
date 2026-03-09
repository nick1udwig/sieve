use super::*;
use crate::automation::parse_at_timestamp_ms;
use crate::config::DEFAULT_POLICY_PATH;
use crate::lcm_integration::LcmIntegrationConfig;
use crate::logging::now_ms;
use sieve_runtime::Clock;
use sieve_types::{
    AutomationAction, AutomationRequest, AutomationSchedule, AutomationTarget, UncertainMode,
    UnknownMode,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicU64;

struct TestClock {
    now_ms: AtomicU64,
}

impl TestClock {
    fn new(now_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(now_ms),
        }
    }

    fn set(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::Relaxed);
    }
}

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

fn test_cfg(root: &Path) -> AppConfig {
    AppConfig {
        telegram_bot_token: "test-token".to_string(),
        telegram_chat_id: 42,
        telegram_poll_timeout_secs: 1,
        telegram_allowed_sender_user_ids: Some(BTreeSet::from([1001])),
        sieve_home: root.to_path_buf(),
        policy_path: PathBuf::from(DEFAULT_POLICY_PATH),
        event_log_path: root.join("logs/runtime-events.jsonl"),
        automation_store_path: root.join("state/automation.json"),
        codex_store_path: root.join("state/codex.db"),
        runtime_cwd: root.to_string_lossy().to_string(),
        heartbeat_interval_ms: None,
        heartbeat_prompt_override: Some("Review pending reminders".to_string()),
        heartbeat_file_path: root.join("HEARTBEAT.md"),
        allowed_tools: vec!["bash".to_string()],
        allowed_net_connect_scopes: Vec::new(),
        unknown_mode: UnknownMode::Deny,
        uncertain_mode: UncertainMode::Deny,
        max_concurrent_turns: 1,
        max_planner_steps: 3,
        max_summary_calls_per_turn: 12,
        lcm: {
            let mut lcm = LcmIntegrationConfig::from_sieve_home(root);
            lcm.enabled = false;
            lcm
        },
    }
}

fn unique_root(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}-{}", std::process::id(), now_ms()))
}

fn ts(raw: &str) -> u64 {
    parse_at_timestamp_ms(raw).expect("valid RFC3339")
}

#[tokio::test]
async fn handle_command_adds_and_lists_jobs() {
    let root = unique_root("automation-manager-command");
    fs::create_dir_all(&root).expect("create root");
    let (prompt_tx, _prompt_rx) = tokio_mpsc::unbounded_channel();
    let clock = Arc::new(TestClock::new(ts("2026-03-05T10:00:00Z")));
    let manager = AutomationManager::new(&test_cfg(&root), prompt_tx, clock).expect("manager");

    let added = manager
        .handle_command("/cron add main every 15m -- remind me to check build")
        .await
        .expect("handle add")
        .expect("reply");
    assert!(added.contains("cron added: cron-1"));

    let listed = manager
        .handle_command("/cron list")
        .await
        .expect("handle list")
        .expect("reply");
    assert!(listed.contains("cron-1 main every 15m"));
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn process_ready_enqueues_heartbeat_for_due_main_cron_job() {
    let root = unique_root("automation-manager-main-cron");
    fs::create_dir_all(&root).expect("create root");
    let (prompt_tx, mut prompt_rx) = tokio_mpsc::unbounded_channel();
    let clock = Arc::new(TestClock::new(ts("2026-03-05T10:00:00Z")));
    let manager =
        AutomationManager::new(&test_cfg(&root), prompt_tx, clock.clone()).expect("manager");

    manager
        .handle_command("/cron add main at 2026-03-05T10:00:01Z -- Reminder: check deploys")
        .await
        .expect("handle add");
    clock.set(ts("2026-03-05T10:00:01Z"));
    manager.process_ready().await.expect("process ready");

    let prompt = prompt_rx.recv().await.expect("heartbeat prompt");
    assert_eq!(prompt.source, PromptSource::Automation);
    assert_eq!(prompt.session_key, MAIN_SESSION_KEY);
    assert!(matches!(prompt.turn_kind, TurnKind::Heartbeat { .. }));
    assert!(prompt.text.contains("Reminder: check deploys"));
    let _ = fs::remove_dir_all(root);
}

#[tokio::test]
async fn handle_tool_request_supports_cron_lifecycle_actions() {
    let root = unique_root("automation-manager-tool");
    fs::create_dir_all(&root).expect("create root");
    let (prompt_tx, _prompt_rx) = tokio_mpsc::unbounded_channel();
    let clock = Arc::new(TestClock::new(ts("2026-03-05T10:00:00Z")));
    let manager = AutomationManager::new(&test_cfg(&root), prompt_tx, clock).expect("manager");

    let added = manager
        .handle_tool_request(AutomationRequest {
            action: AutomationAction::CronAdd,
            target: Some(AutomationTarget::Main),
            schedule: Some(AutomationSchedule::Every {
                interval: "15m".to_string(),
            }),
            prompt: Some("remind me to check build".to_string()),
            job_id: None,
        })
        .await
        .expect("add request");
    assert!(added.message.contains("cron added: cron-1"));

    let listed = manager
        .handle_tool_request(AutomationRequest {
            action: AutomationAction::CronList,
            target: None,
            schedule: None,
            prompt: None,
            job_id: None,
        })
        .await
        .expect("list request");
    assert!(listed.message.contains("cron-1 main every 15m"));

    let paused = manager
        .handle_tool_request(AutomationRequest {
            action: AutomationAction::CronPause,
            target: None,
            schedule: None,
            prompt: None,
            job_id: Some("cron-1".to_string()),
        })
        .await
        .expect("pause request");
    assert_eq!(paused.message, "cron paused: cron-1");

    let resumed = manager
        .handle_tool_request(AutomationRequest {
            action: AutomationAction::CronResume,
            target: None,
            schedule: None,
            prompt: None,
            job_id: Some("cron-1".to_string()),
        })
        .await
        .expect("resume request");
    assert!(resumed.message.contains("cron resumed: cron-1 next"));

    let removed = manager
        .handle_tool_request(AutomationRequest {
            action: AutomationAction::CronRemove,
            target: None,
            schedule: None,
            prompt: None,
            job_id: Some("cron-1".to_string()),
        })
        .await
        .expect("remove request");
    assert_eq!(removed.message, "cron removed: cron-1");
    let _ = fs::remove_dir_all(root);
}
