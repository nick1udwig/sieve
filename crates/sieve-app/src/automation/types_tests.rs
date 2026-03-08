use super::*;
use chrono::DateTime;

fn ts(raw: &str) -> u64 {
    DateTime::parse_from_rfc3339(raw)
        .expect("valid RFC3339")
        .timestamp_millis() as u64
}

#[test]
fn parse_duration_ms_supports_compound_units() {
    assert_eq!(parse_duration_ms("90s").expect("90s"), 90_000);
    assert_eq!(parse_duration_ms("1h30m").expect("1h30m"), 5_400_000);
    assert_eq!(parse_duration_ms("2d4h").expect("2d4h"), 187_200_000);
}

#[test]
fn parse_duration_ms_rejects_bad_input() {
    assert!(parse_duration_ms("").is_err());
    assert!(parse_duration_ms("10").is_err());
    assert!(parse_duration_ms("abc").is_err());
    assert!(parse_duration_ms("0s").is_err());
}

#[test]
fn parse_at_timestamp_ms_accepts_rfc3339_and_unix_ms() {
    assert_eq!(
        parse_at_timestamp_ms("1717171717000").expect("unix ms"),
        1_717_171_717_000
    );
    assert_eq!(
        parse_at_timestamp_ms("2026-03-05T10:00:00Z").expect("rfc3339"),
        ts("2026-03-05T10:00:00Z")
    );
}

#[test]
fn enqueue_system_event_dedupes_same_text_and_context() {
    let mut store = AutomationStore::default();
    assert!(store.enqueue_system_event(MAIN_SESSION_KEY, "Reminder", Some("cron:a"), 10));
    assert!(!store.enqueue_system_event(MAIN_SESSION_KEY, "Reminder", Some("cron:a"), 11));
    assert!(store.enqueue_system_event(MAIN_SESSION_KEY, "Reminder", Some("cron:b"), 12));
    assert_eq!(store.peek_system_events(MAIN_SESSION_KEY).len(), 2);
}

#[test]
fn ack_system_events_removes_only_targeted_ids() {
    let mut store = AutomationStore::default();
    store.enqueue_system_event(MAIN_SESSION_KEY, "A", Some("cron:a"), 10);
    store.enqueue_system_event(MAIN_SESSION_KEY, "B", Some("cron:b"), 11);
    let events = store.peek_system_events(MAIN_SESSION_KEY);
    store.ack_system_events(MAIN_SESSION_KEY, &[events[0].id.clone()]);
    let remaining = store.peek_system_events(MAIN_SESSION_KEY);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].text, "B");
}

#[test]
fn add_cron_job_sets_next_due_for_every_schedule() {
    let mut store = AutomationStore::default();
    let job = store
        .add_cron_job(
            CronSessionTarget::Main,
            CronJobSchedule::Every {
                every_ms: 60_000,
                anchor_ms: 1_000,
            },
            "check inbox".to_string(),
            1_500,
        )
        .expect("add every job");
    assert_eq!(job.next_run_at_ms, Some(61_000));
}

#[test]
fn mark_job_finished_disables_one_shot_at_jobs() {
    let mut store = AutomationStore::default();
    let job = store
        .add_cron_job(
            CronSessionTarget::Isolated,
            CronJobSchedule::At { at_ms: 5_000 },
            "run report".to_string(),
            1_000,
        )
        .expect("add at job");
    store.mark_job_started(&job.id, 5_000).expect("start job");
    let finished = store
        .mark_job_finished(&job.id, 5_100, CronJobStatus::Succeeded, None)
        .expect("finish job");
    assert!(!finished.enabled);
    assert_eq!(finished.next_run_at_ms, None);
}

#[test]
fn heartbeat_due_at_uses_last_run_anchor() {
    let mut store = AutomationStore::default();
    assert_eq!(store.heartbeat_due_at_ms(Some(60_000), 1_000), Some(61_000));
    store.record_heartbeat_run(5_000, None);
    assert_eq!(store.heartbeat_due_at_ms(Some(60_000), 9_000), Some(65_000));
}

#[test]
fn cron_schedule_produces_future_occurrence() {
    let schedule = CronJobSchedule::Cron {
        expr: "*/15 * * * *".to_string(),
    };
    let next = schedule
        .next_run_at_ms(ts("2026-03-05T10:00:00Z"))
        .expect("next cron due")
        .expect("cron due");
    assert!(next > ts("2026-03-05T10:00:00Z"));
}
