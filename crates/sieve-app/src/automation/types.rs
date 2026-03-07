use chrono::{DateTime, Local, TimeZone, Utc};
use croner::Cron;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) const DEFAULT_HEARTBEAT_FILE_NAME: &str = "HEARTBEAT.md";
pub(crate) const HEARTBEAT_OK_TOKEN: &str = "HEARTBEAT_OK";
pub(crate) const MAIN_SESSION_KEY: &str = "main";
const AUTOMATION_STORE_SCHEMA_VERSION: u16 = 1;
static AUTOMATION_STORE_TMP_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CronSessionTarget {
    Main,
    Isolated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CronJobSchedule {
    At { at_ms: u64 },
    Every { every_ms: u64, anchor_ms: u64 },
    Cron { expr: String },
}

impl CronJobSchedule {
    pub(crate) fn initial_next_run_at_ms(&self, now_ms: u64) -> Result<Option<u64>, String> {
        self.next_run_at_ms(now_ms)
    }

    pub(crate) fn next_run_at_ms(&self, now_ms: u64) -> Result<Option<u64>, String> {
        match self {
            Self::At { at_ms } => Ok((*at_ms >= now_ms).then_some(*at_ms)),
            Self::Every {
                every_ms,
                anchor_ms,
            } => Ok(Some(next_every_due_ms(*anchor_ms, *every_ms, now_ms)?)),
            Self::Cron { expr } => next_cron_due_ms(expr, now_ms).map(Some),
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::At { at_ms } => format!("at {}", render_timestamp_ms(*at_ms)),
            Self::Every { every_ms, .. } => format!("every {}", render_duration_ms(*every_ms)),
            Self::Cron { expr } => format!("cron {expr}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CronJobStatus {
    QueuedMain,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CronJob {
    pub(crate) id: String,
    pub(crate) target: CronSessionTarget,
    pub(crate) schedule: CronJobSchedule,
    pub(crate) prompt: String,
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) announce_to_main: bool,
    pub(crate) created_at_ms: u64,
    pub(crate) updated_at_ms: u64,
    pub(crate) next_run_at_ms: Option<u64>,
    pub(crate) last_started_at_ms: Option<u64>,
    pub(crate) last_finished_at_ms: Option<u64>,
    pub(crate) last_status: Option<CronJobStatus>,
    pub(crate) last_error: Option<String>,
    #[serde(default)]
    pub(crate) running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QueuedSystemEvent {
    pub(crate) id: String,
    pub(crate) text: String,
    pub(crate) created_at_ms: u64,
    pub(crate) context_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) struct AutomationSessionState {
    #[serde(default)]
    pub(crate) queued_system_events: Vec<QueuedSystemEvent>,
    pub(crate) last_system_event_text: Option<String>,
    pub(crate) last_system_event_context_key: Option<String>,
    pub(crate) last_heartbeat_run_at_ms: Option<u64>,
    pub(crate) last_heartbeat_message: Option<String>,
    pub(crate) last_heartbeat_delivered_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AutomationStore {
    pub(crate) schema_version: u16,
    pub(crate) next_job_nonce: u64,
    pub(crate) next_event_nonce: u64,
    #[serde(default)]
    pub(crate) sessions: BTreeMap<String, AutomationSessionState>,
    #[serde(default)]
    pub(crate) cron_jobs: BTreeMap<String, CronJob>,
}

impl Default for AutomationStore {
    fn default() -> Self {
        Self {
            schema_version: AUTOMATION_STORE_SCHEMA_VERSION,
            next_job_nonce: 1,
            next_event_nonce: 1,
            sessions: BTreeMap::new(),
            cron_jobs: BTreeMap::new(),
        }
    }
}

impl AutomationStore {
    pub(crate) fn main_session(&self) -> AutomationSessionState {
        self.sessions
            .get(MAIN_SESSION_KEY)
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn heartbeat_due_at_ms(&self, interval_ms: Option<u64>, now_ms: u64) -> Option<u64> {
        let interval_ms = interval_ms?;
        let session = self.main_session();
        Some(
            session
                .last_heartbeat_run_at_ms
                .map(|last| last.saturating_add(interval_ms))
                .unwrap_or_else(|| now_ms.saturating_add(interval_ms)),
        )
    }

    pub(crate) fn peek_system_events(&self, session_key: &str) -> Vec<QueuedSystemEvent> {
        self.sessions
            .get(session_key)
            .map(|session| session.queued_system_events.clone())
            .unwrap_or_default()
    }

    pub(crate) fn enqueue_system_event(
        &mut self,
        session_key: &str,
        text: &str,
        context_key: Option<&str>,
        now_ms: u64,
    ) -> bool {
        let cleaned = text.trim();
        if cleaned.is_empty() {
            return false;
        }
        let normalized_context_key = normalize_context_key(context_key);
        let session = self.sessions.entry(session_key.to_string()).or_default();
        if session.last_system_event_text.as_deref() == Some(cleaned)
            && session.last_system_event_context_key == normalized_context_key
        {
            return false;
        }
        let event_id = format!("evt-{}", self.next_event_nonce);
        self.next_event_nonce = self.next_event_nonce.saturating_add(1);
        session.last_system_event_text = Some(cleaned.to_string());
        session.last_system_event_context_key = normalized_context_key.clone();
        session.queued_system_events.push(QueuedSystemEvent {
            id: event_id,
            text: cleaned.to_string(),
            created_at_ms: now_ms,
            context_key: normalized_context_key,
        });
        true
    }

    pub(crate) fn ack_system_events(&mut self, session_key: &str, event_ids: &[String]) {
        if event_ids.is_empty() {
            return;
        }
        let Some(session) = self.sessions.get_mut(session_key) else {
            return;
        };
        session
            .queued_system_events
            .retain(|event| !event_ids.iter().any(|id| id == &event.id));
        if session.queued_system_events.is_empty() {
            session.last_system_event_text = None;
            session.last_system_event_context_key = None;
        }
    }

    pub(crate) fn record_heartbeat_run(&mut self, now_ms: u64, delivered_message: Option<String>) {
        let session = self
            .sessions
            .entry(MAIN_SESSION_KEY.to_string())
            .or_default();
        session.last_heartbeat_run_at_ms = Some(now_ms);
        if let Some(message) = delivered_message {
            session.last_heartbeat_message = Some(message);
            session.last_heartbeat_delivered_at_ms = Some(now_ms);
        }
    }

    pub(crate) fn add_cron_job(
        &mut self,
        target: CronSessionTarget,
        schedule: CronJobSchedule,
        prompt: String,
        now_ms: u64,
    ) -> Result<CronJob, String> {
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("cron prompt cannot be empty".to_string());
        }
        let id = format!("cron-{}", self.next_job_nonce);
        self.next_job_nonce = self.next_job_nonce.saturating_add(1);
        let next_run_at_ms = schedule.initial_next_run_at_ms(now_ms)?;
        if matches!(schedule, CronJobSchedule::At { .. }) && next_run_at_ms.is_none() {
            return Err("`at` schedule must be in the future".to_string());
        }
        let job = CronJob {
            id: id.clone(),
            target,
            schedule,
            prompt,
            enabled: true,
            announce_to_main: false,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            next_run_at_ms,
            last_started_at_ms: None,
            last_finished_at_ms: None,
            last_status: None,
            last_error: None,
            running: false,
        };
        self.cron_jobs.insert(id, job.clone());
        Ok(job)
    }

    pub(crate) fn remove_cron_job(&mut self, job_id: &str) -> Option<CronJob> {
        self.cron_jobs.remove(job_id)
    }

    pub(crate) fn pause_cron_job(&mut self, job_id: &str, now_ms: u64) -> Result<(), String> {
        let job = self
            .cron_jobs
            .get_mut(job_id)
            .ok_or_else(|| format!("unknown cron job `{job_id}`"))?;
        job.enabled = false;
        job.updated_at_ms = now_ms;
        job.next_run_at_ms = None;
        Ok(())
    }

    pub(crate) fn resume_cron_job(&mut self, job_id: &str, now_ms: u64) -> Result<(), String> {
        let job = self
            .cron_jobs
            .get_mut(job_id)
            .ok_or_else(|| format!("unknown cron job `{job_id}`"))?;
        job.enabled = true;
        job.updated_at_ms = now_ms;
        job.next_run_at_ms = job.schedule.next_run_at_ms(now_ms)?;
        Ok(())
    }

    pub(crate) fn due_job_ids(&self, now_ms: u64) -> Vec<String> {
        self.cron_jobs
            .values()
            .filter(|job| job.enabled && !job.running)
            .filter_map(|job| {
                job.next_run_at_ms
                    .filter(|next| *next <= now_ms)
                    .map(|_| job.id.clone())
            })
            .collect()
    }

    pub(crate) fn mark_job_started(
        &mut self,
        job_id: &str,
        now_ms: u64,
    ) -> Result<CronJob, String> {
        let job = self
            .cron_jobs
            .get_mut(job_id)
            .ok_or_else(|| format!("unknown cron job `{job_id}`"))?;
        job.running = true;
        job.updated_at_ms = now_ms;
        job.last_started_at_ms = Some(now_ms);
        Ok(job.clone())
    }

    pub(crate) fn mark_job_finished(
        &mut self,
        job_id: &str,
        now_ms: u64,
        status: CronJobStatus,
        error: Option<String>,
    ) -> Result<CronJob, String> {
        let job = self
            .cron_jobs
            .get_mut(job_id)
            .ok_or_else(|| format!("unknown cron job `{job_id}`"))?;
        job.running = false;
        job.updated_at_ms = now_ms;
        job.last_finished_at_ms = Some(now_ms);
        job.last_status = Some(status);
        job.last_error = error;
        job.next_run_at_ms = if job.enabled {
            job.schedule.next_run_at_ms(now_ms)?
        } else {
            None
        };
        if matches!(job.schedule, CronJobSchedule::At { .. }) {
            job.enabled = false;
            job.next_run_at_ms = None;
        }
        Ok(job.clone())
    }
}

pub(crate) fn load_automation_store(path: &Path) -> Result<AutomationStore, String> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(AutomationStore::default()),
        Err(err) => return Err(format!("failed reading {}: {err}", path.display())),
    };
    let parsed: AutomationStore = serde_json::from_str(&body)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    if parsed.schema_version != AUTOMATION_STORE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported automation store schema_version {} in {}",
            parsed.schema_version,
            path.display()
        ));
    }
    Ok(parsed)
}

pub(crate) fn save_automation_store(path: &Path, store: &AutomationStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating {}: {err}", parent.display()))?;
    }
    let encoded = serde_json::to_string_pretty(store)
        .map_err(|err| format!("failed encoding automation store: {err}"))?;
    let nonce = AUTOMATION_STORE_TMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), nonce));
    fs::write(&tmp_path, encoded)
        .map_err(|err| format!("failed writing {}: {err}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).map_err(|err| {
        format!(
            "failed renaming {} to {}: {err}",
            tmp_path.display(),
            path.display()
        )
    })
}

pub(crate) fn parse_duration_ms(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("duration cannot be empty".to_string());
    }

    let mut total_ms = 0u64;
    let mut idx = 0usize;
    let bytes = trimmed.as_bytes();
    while idx < bytes.len() {
        let num_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if num_start == idx {
            return Err(format!("invalid duration segment in `{trimmed}`"));
        }
        let value = trimmed[num_start..idx]
            .parse::<u64>()
            .map_err(|err| format!("invalid duration number in `{trimmed}`: {err}"))?;
        let unit_start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
            idx += 1;
        }
        let unit = &trimmed[unit_start..idx];
        let factor_ms = match unit {
            "ms" => 1,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 60 * 60_000,
            "d" => 24 * 60 * 60_000,
            "w" => 7 * 24 * 60 * 60_000,
            "" => return Err(format!("duration segment missing unit in `{trimmed}`")),
            _ => return Err(format!("unsupported duration unit `{unit}` in `{trimmed}`")),
        };
        total_ms = total_ms
            .checked_add(
                value
                    .checked_mul(factor_ms)
                    .ok_or_else(|| format!("duration overflow in `{trimmed}`"))?,
            )
            .ok_or_else(|| format!("duration overflow in `{trimmed}`"))?;
    }

    if total_ms == 0 {
        return Err("duration must be greater than zero".to_string());
    }
    Ok(total_ms)
}

pub(crate) fn parse_at_timestamp_ms(raw: &str) -> Result<u64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("timestamp cannot be empty".to_string());
    }
    if let Ok(unix_ms) = trimmed.parse::<u64>() {
        return Ok(unix_ms);
    }
    DateTime::parse_from_rfc3339(trimmed)
        .map(|dt| dt.timestamp_millis() as u64)
        .map_err(|err| format!("invalid RFC3339 timestamp `{trimmed}`: {err}"))
}

fn next_every_due_ms(anchor_ms: u64, every_ms: u64, now_ms: u64) -> Result<u64, String> {
    if every_ms == 0 {
        return Err("every schedule requires duration > 0".to_string());
    }
    if now_ms <= anchor_ms {
        return Ok(anchor_ms);
    }
    let elapsed = now_ms.saturating_sub(anchor_ms);
    let intervals = elapsed / every_ms;
    let candidate = anchor_ms
        .checked_add(intervals.saturating_mul(every_ms))
        .ok_or_else(|| "every schedule overflow".to_string())?;
    if candidate > now_ms {
        Ok(candidate)
    } else {
        candidate
            .checked_add(every_ms)
            .ok_or_else(|| "every schedule overflow".to_string())
    }
}

fn next_cron_due_ms(expr: &str, now_ms: u64) -> Result<u64, String> {
    let cron =
        Cron::from_str(expr).map_err(|err| format!("invalid cron expression `{expr}`: {err}"))?;
    let now = Local
        .timestamp_millis_opt(now_ms as i64)
        .single()
        .ok_or_else(|| format!("invalid local timestamp `{now_ms}`"))?;
    let next = cron
        .find_next_occurrence(&now, false)
        .map_err(|err| format!("failed resolving next cron occurrence for `{expr}`: {err}"))?;
    let next_utc = next.with_timezone(&Utc);
    Ok(next_utc.timestamp_millis() as u64)
}

fn normalize_context_key(context_key: Option<&str>) -> Option<String> {
    context_key
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(|key| key.to_ascii_lowercase())
}

fn render_duration_ms(duration_ms: u64) -> String {
    if duration_ms % (24 * 60 * 60_000) == 0 {
        return format!("{}d", duration_ms / (24 * 60 * 60_000));
    }
    if duration_ms % (60 * 60_000) == 0 {
        return format!("{}h", duration_ms / (60 * 60_000));
    }
    if duration_ms % 60_000 == 0 {
        return format!("{}m", duration_ms / 60_000);
    }
    if duration_ms % 1_000 == 0 {
        return format!("{}s", duration_ms / 1_000);
    }
    format!("{duration_ms}ms")
}

fn render_timestamp_ms(timestamp_ms: u64) -> String {
    Utc.timestamp_millis_opt(timestamp_ms as i64)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| timestamp_ms.to_string())
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod types_tests;
