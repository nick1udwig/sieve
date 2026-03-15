use super::*;

struct EnvRestore {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvRestore {
    fn capture(keys: &[&'static str]) -> Self {
        Self {
            saved: keys
                .iter()
                .map(|key| (*key, std::env::var(key).ok()))
                .collect(),
        }
    }

    fn clear(&self) {
        for (key, _) in &self.saved {
            std::env::remove_var(key);
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn config_env_keys() -> &'static [&'static str] {
    &[
        "TELEGRAM_BOT_TOKEN",
        "TELEGRAM_CHAT_ID",
        "SIEVE_TELEGRAM_POLL_TIMEOUT_SECS",
        "SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS",
        "SIEVE_POLICY_PATH",
        "SIEVE_HOME",
        "HOME",
        "SIEVE_RUNTIME_CWD",
        "SIEVE_HEARTBEAT_EVERY",
        "SIEVE_HEARTBEAT_PROMPT",
        "SIEVE_ALLOWED_TOOLS",
        "SIEVE_UNKNOWN_MODE",
        "SIEVE_UNCERTAIN_MODE",
        "SIEVE_MAX_CONCURRENT_TURNS",
        "SIEVE_MAX_PLANNER_STEPS",
        "SIEVE_MAX_SUMMARY_CALLS_PER_TURN",
        "SIEVE_LCM_ENABLED",
        "SIEVE_LCM_GLOBAL_SESSION_ID",
        "SIEVE_LCM_TRUSTED_DB_PATH",
        "SIEVE_LCM_UNTRUSTED_DB_PATH",
        "SIEVE_LCM_CLI_BIN",
    ]
}
#[test]
fn modality_contract_defaults_and_overrides() {
    let mut contract = default_modality_contract(InteractionModality::Audio);
    assert_eq!(contract.input, InteractionModality::Audio);
    assert_eq!(contract.response, InteractionModality::Audio);
    assert!(contract.override_reason.is_none());

    override_modality_contract(
        &mut contract,
        InteractionModality::Text,
        ModalityOverrideReason::ToolFailure,
    );
    assert_eq!(contract.response, InteractionModality::Text);
    assert_eq!(
        contract.override_reason,
        Some(ModalityOverrideReason::ToolFailure)
    );
}

#[test]
fn parse_policy_path_uses_baseline_default_for_missing_or_blank() {
    assert_eq!(
        parse_policy_path(None),
        PathBuf::from("docs/policy/baseline-policy.toml")
    );
    assert_eq!(
        parse_policy_path(Some(String::new())),
        PathBuf::from("docs/policy/baseline-policy.toml")
    );
    assert_eq!(
        parse_policy_path(Some("   ".to_string())),
        PathBuf::from("docs/policy/baseline-policy.toml")
    );
}

#[test]
fn parse_policy_path_honors_explicit_env_override() {
    assert_eq!(
        parse_policy_path(Some("custom/policy.toml".to_string())),
        PathBuf::from("custom/policy.toml")
    );
}

#[test]
fn planner_allowed_tools_for_turn_hides_explicit_ref_tools_without_value_refs() {
    let configured = vec![
        "bash".to_string(),
        "gws".to_string(),
        "automation".to_string(),
        "codex_exec".to_string(),
        "codex_session".to_string(),
        "endorse".to_string(),
        "declassify".to_string(),
    ];
    assert_eq!(
        planner_allowed_tools_for_turn(&configured, false, false, false),
        vec!["bash".to_string()]
    );
    assert_eq!(
        planner_allowed_tools_for_turn(&configured, false, true, false),
        vec!["bash".to_string(), "automation".to_string()]
    );
    assert_eq!(
        planner_allowed_tools_for_turn(&configured, true, false, false),
        vec![
            "bash".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ]
    );
    assert_eq!(
        planner_allowed_tools_for_turn(&configured, true, true, true),
        vec![
            "bash".to_string(),
            "automation".to_string(),
            "codex_exec".to_string(),
            "codex_session".to_string(),
            "endorse".to_string(),
            "declassify".to_string(),
        ]
    );
}

#[test]
fn planner_allowed_net_connect_scopes_prefers_origin_level_entries() {
    let policy = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://forecast.weather.gov/MapClick.php?lat=37.7&lon=-122.4"

[[allow_capabilities]]
resource = "net"
action = "connect"
scope = "https://forecast.weather.gov/hourly"

[[allow_capabilities]]
resource = "fs"
action = "read"
scope = "/tmp/input.txt"
"#,
    )
    .expect("parse policy");

    let scopes = planner_allowed_net_connect_scopes(&policy);
    assert_eq!(scopes, vec!["https://forecast.weather.gov".to_string()]);
}

#[test]
fn parse_event_log_path_defaults_to_home_sieve_logs() {
    assert_eq!(
        runtime_event_log_path(&parse_sieve_home(None, Some("/home/alice".to_string()))),
        PathBuf::from("/home/alice/.sieve/logs/runtime-events.jsonl")
    );
}

#[test]
fn parse_event_log_path_uses_sieve_home_when_set() {
    assert_eq!(
        runtime_event_log_path(&parse_sieve_home(
            Some("/var/sieve".to_string()),
            Some("/home/alice".to_string())
        )),
        PathBuf::from("/var/sieve/logs/runtime-events.jsonl")
    );
}

#[test]
fn automation_store_path_uses_sieve_home_state_dir() {
    assert_eq!(
        crate::config::automation_store_path(Path::new("/var/sieve")),
        PathBuf::from("/var/sieve/state/automation.json")
    );
}

#[test]
fn load_approval_allowances_missing_file_returns_empty() {
    let path = std::env::temp_dir().join(format!(
        "sieve-app-allowances-missing-{}-{}.json",
        std::process::id(),
        now_ms()
    ));
    let loaded = load_approval_allowances(&path).expect("missing file should be empty");
    assert!(loaded.is_empty());
}

#[test]
fn approval_allowances_file_round_trip() {
    let root = std::env::temp_dir().join(format!(
        "sieve-app-allowances-roundtrip-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let path = approval_allowances_path(&root);
    let allowances = vec![
        Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "https://example.com".to_string(),
        },
        Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: "/tmp/input.txt".to_string(),
        },
    ];
    save_approval_allowances(&path, &allowances).expect("save allowances");
    let loaded = load_approval_allowances(&path).expect("load allowances");
    assert_eq!(loaded, allowances);
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn approval_allowances_parallel_saves_do_not_fail() {
    let root = std::env::temp_dir().join(format!(
        "sieve-app-allowances-parallel-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let path = approval_allowances_path(&root);
    let workers = 16usize;
    let rounds_per_worker = 12usize;
    let start = Arc::new(std::sync::Barrier::new(workers));
    let errors = Arc::new(StdMutex::new(Vec::new()));

    std::thread::scope(|scope| {
        for worker_idx in 0..workers {
            let path = path.clone();
            let start = Arc::clone(&start);
            let errors = Arc::clone(&errors);
            scope.spawn(move || {
                start.wait();
                for round in 0..rounds_per_worker {
                    let allowances = vec![Capability {
                        resource: Resource::Fs,
                        action: Action::Read,
                        scope: format!("/tmp/input-{worker_idx}-{round}.txt"),
                    }];
                    if let Err(err) = save_approval_allowances(&path, &allowances) {
                        errors.lock().expect("errors lock").push(err);
                    }
                }
            });
        }
    });

    let failures = errors.lock().expect("errors lock").clone();
    assert!(
        failures.is_empty(),
        "parallel save failures: {}",
        failures.join("; ")
    );
    let loaded = load_approval_allowances(&path).expect("load final allowances");
    assert!(!loaded.is_empty(), "final allowances must exist");
    let _ = fs::remove_file(&path);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_telegram_allowed_sender_user_ids_supports_missing_and_blank() {
    assert_eq!(parse_telegram_allowed_sender_user_ids(None), Ok(None));
    assert_eq!(
        parse_telegram_allowed_sender_user_ids(Some("   ".to_string())),
        Ok(None)
    );
}

#[test]
fn parse_telegram_allowed_sender_user_ids_parses_csv() {
    let parsed = parse_telegram_allowed_sender_user_ids(Some("1001,-42,1001".to_string()))
        .expect("parse ids");
    assert_eq!(parsed, Some(BTreeSet::from([1001, -42])));
}

#[test]
fn parse_telegram_allowed_sender_user_ids_rejects_invalid_entry() {
    let err = parse_telegram_allowed_sender_user_ids(Some("1001,nope".to_string()))
        .expect_err("must reject invalid user id");
    assert!(err.contains("invalid SIEVE_TELEGRAM_ALLOWED_SENDER_USER_IDS entry `nope`"));
}

#[test]
fn app_config_from_env_loads_heartbeat_settings() {
    let _lock = env_test_lock().lock().expect("env lock");
    let restore = EnvRestore::capture(config_env_keys());
    restore.clear();

    std::env::set_var("TELEGRAM_BOT_TOKEN", "test-bot-token");
    std::env::set_var("TELEGRAM_CHAT_ID", "42");
    std::env::set_var("SIEVE_HOME", "/tmp/sieve-config-home");
    std::env::set_var("SIEVE_RUNTIME_CWD", "/tmp/sieve-runtime");
    std::env::set_var("SIEVE_HEARTBEAT_EVERY", "15m");
    std::env::set_var("SIEVE_HEARTBEAT_PROMPT", "Check queued reminders.");
    std::env::set_var("SIEVE_LCM_ENABLED", "0");

    let cfg = AppConfig::from_env().expect("config");
    assert_eq!(
        cfg.automation_store_path,
        PathBuf::from("/tmp/sieve-config-home/state/automation.json")
    );
    assert_eq!(cfg.heartbeat_interval_ms, Some(900_000));
    assert_eq!(
        cfg.heartbeat_prompt_override,
        Some("Check queued reminders.".to_string())
    );
    assert_eq!(
        cfg.heartbeat_file_path,
        PathBuf::from("/tmp/sieve-runtime/HEARTBEAT.md")
    );
}

#[test]
fn app_config_from_env_rejects_invalid_heartbeat_duration() {
    let _lock = env_test_lock().lock().expect("env lock");
    let restore = EnvRestore::capture(config_env_keys());
    restore.clear();

    std::env::set_var("TELEGRAM_BOT_TOKEN", "test-bot-token");
    std::env::set_var("TELEGRAM_CHAT_ID", "42");
    std::env::set_var("SIEVE_HEARTBEAT_EVERY", "15x");

    let err = AppConfig::from_env()
        .err()
        .expect("invalid heartbeat duration");
    assert!(err.contains("invalid SIEVE_HEARTBEAT_EVERY"));
}
