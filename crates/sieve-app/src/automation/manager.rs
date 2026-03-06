use super::heartbeat::build_heartbeat_prompt;
use super::{
    load_automation_store, parse_automation_command, save_automation_store, AutomationCommand,
    AutomationStore, CronJob, CronJobStatus, CronSessionTarget, MAIN_SESSION_KEY,
};
use crate::config::AppConfig;
use crate::ingress::{IngressPrompt, PromptSource, TurnKind};
use crate::turn::TurnOutcome;
use chrono::TimeZone;
use sieve_runtime::Clock;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc as tokio_mpsc, Mutex, Notify};

pub(crate) struct AutomationManager {
    store_path: PathBuf,
    store: Mutex<AutomationStore>,
    prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
    clock: Arc<dyn Clock>,
    heartbeat_interval_ms: Option<u64>,
    heartbeat_prompt_override: Option<String>,
    heartbeat_file_path: PathBuf,
    notify: Notify,
    pending_heartbeat_reason: Mutex<Option<String>>,
    main_turns_in_flight: AtomicUsize,
    heartbeat_running: AtomicBool,
}

impl AutomationManager {
    pub(crate) fn new(
        cfg: &AppConfig,
        prompt_tx: tokio_mpsc::UnboundedSender<IngressPrompt>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, String> {
        Ok(Self {
            store_path: cfg.automation_store_path.clone(),
            store: Mutex::new(load_automation_store(&cfg.automation_store_path)?),
            prompt_tx,
            clock,
            heartbeat_interval_ms: cfg.heartbeat_interval_ms,
            heartbeat_prompt_override: cfg.heartbeat_prompt_override.clone(),
            heartbeat_file_path: cfg.heartbeat_file_path.clone(),
            notify: Notify::new(),
            pending_heartbeat_reason: Mutex::new(None),
            main_turns_in_flight: AtomicUsize::new(0),
            heartbeat_running: AtomicBool::new(false),
        })
    }

    pub(crate) async fn handle_command(&self, input: &str) -> Result<Option<String>, String> {
        let now_ms = self.clock.now_ms();
        let Some(command) = parse_automation_command(input, now_ms)? else {
            return Ok(None);
        };

        match command {
            AutomationCommand::HeartbeatNow => {
                self.request_heartbeat_now(Some("manual".to_string())).await;
                Ok(Some("heartbeat queued".to_string()))
            }
            AutomationCommand::CronList => {
                let jobs = {
                    let store = self.store.lock().await;
                    store.cron_jobs.values().cloned().collect::<Vec<CronJob>>()
                };
                Ok(Some(format_cron_job_list(&jobs)))
            }
            AutomationCommand::CronAdd {
                target,
                schedule,
                prompt,
            } => {
                let job = self
                    .mutate_store(|store, now_ms| {
                        store.add_cron_job(target, schedule, prompt, now_ms)
                    })
                    .await?;
                self.notify.notify_waiters();
                Ok(Some(format!(
                    "cron added: {} {} {}",
                    job.id,
                    target_label(&job.target),
                    job.schedule.describe()
                )))
            }
            AutomationCommand::CronRemove { job_id } => {
                let removed = self
                    .mutate_store(|store, _| {
                        store
                            .remove_cron_job(&job_id)
                            .ok_or_else(|| format!("unknown cron job `{job_id}`"))
                    })
                    .await?;
                self.notify.notify_waiters();
                Ok(Some(format!("cron removed: {}", removed.id)))
            }
            AutomationCommand::CronPause { job_id } => {
                self.mutate_store(|store, now_ms| {
                    store.pause_cron_job(&job_id, now_ms)?;
                    store
                        .cron_jobs
                        .get(&job_id)
                        .cloned()
                        .ok_or_else(|| format!("unknown cron job `{job_id}`"))
                })
                .await?;
                self.notify.notify_waiters();
                Ok(Some(format!("cron paused: {job_id}")))
            }
            AutomationCommand::CronResume { job_id } => {
                let resumed = self
                    .mutate_store(|store, now_ms| {
                        store.resume_cron_job(&job_id, now_ms)?;
                        store
                            .cron_jobs
                            .get(&job_id)
                            .cloned()
                            .ok_or_else(|| format!("unknown cron job `{job_id}`"))
                    })
                    .await?;
                self.notify.notify_waiters();
                Ok(Some(format!(
                    "cron resumed: {} next {}",
                    resumed.id,
                    render_optional_timestamp(resumed.next_run_at_ms)
                )))
            }
        }
    }

    pub(crate) async fn run_loop(self: Arc<Self>) {
        loop {
            if let Err(err) = self.process_ready().await {
                eprintln!("automation loop error: {err}");
            }

            let now_ms = self.clock.now_ms();
            let next_due_ms = self.next_due_ms(now_ms).await;
            match next_due_ms {
                Some(next_due_ms) if next_due_ms <= now_ms => {
                    continue;
                }
                Some(next_due_ms) => {
                    tokio::select! {
                        _ = tokio::time::sleep(tokio::time::Duration::from_millis(next_due_ms.saturating_sub(now_ms))) => {}
                        _ = self.notify.notified() => {}
                    }
                }
                None => {
                    self.notify.notified().await;
                }
            }
        }
    }

    pub(crate) fn note_turn_started(&self, prompt: &IngressPrompt) {
        if prompt.session_key == MAIN_SESSION_KEY {
            self.main_turns_in_flight.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) async fn note_turn_finished(
        &self,
        prompt: &IngressPrompt,
        outcome: Option<&TurnOutcome>,
        error: Option<String>,
    ) {
        let now_ms = self.clock.now_ms();
        if prompt.session_key == MAIN_SESSION_KEY {
            self.main_turns_in_flight.fetch_sub(1, Ordering::Relaxed);
        }

        match (&prompt.turn_kind, outcome, error) {
            (
                TurnKind::Heartbeat {
                    queued_event_ids, ..
                },
                Some(turn_outcome),
                _,
            ) => {
                self.heartbeat_running.store(false, Ordering::Relaxed);
                if let Err(err) = self
                    .mutate_store(|store, _| {
                        store.ack_system_events(MAIN_SESSION_KEY, queued_event_ids);
                        store.record_heartbeat_run(
                            now_ms,
                            turn_outcome
                                .assistant_delivered
                                .then(|| turn_outcome.assistant_message.clone()),
                        );
                        Ok(())
                    })
                    .await
                {
                    eprintln!("heartbeat completion persist failed: {err}");
                }
            }
            (TurnKind::Heartbeat { .. }, None, _) => {
                self.heartbeat_running.store(false, Ordering::Relaxed);
                self.request_heartbeat_now(Some("retry".to_string())).await;
            }
            (TurnKind::CronIsolated { job_id }, Some(_), _) => {
                if let Err(err) = self
                    .mutate_store(|store, _| {
                        store.mark_job_finished(job_id, now_ms, CronJobStatus::Succeeded, None)?;
                        Ok(())
                    })
                    .await
                {
                    eprintln!("isolated cron completion persist failed: {err}");
                }
            }
            (TurnKind::CronIsolated { job_id }, None, Some(err)) => {
                if let Err(persist_err) = self
                    .mutate_store(|store, _| {
                        store.mark_job_finished(
                            job_id,
                            now_ms,
                            CronJobStatus::Failed,
                            Some(err),
                        )?;
                        Ok(())
                    })
                    .await
                {
                    eprintln!("isolated cron failure persist failed: {persist_err}");
                }
            }
            _ => {}
        }
        self.notify.notify_waiters();
    }

    pub(crate) async fn request_heartbeat_now(&self, reason: Option<String>) {
        *self.pending_heartbeat_reason.lock().await = reason;
        self.notify.notify_waiters();
    }

    async fn next_due_ms(&self, now_ms: u64) -> Option<u64> {
        if self.heartbeat_running.load(Ordering::Relaxed) {
            return None;
        }
        if self.pending_heartbeat_reason.lock().await.is_some() {
            return Some(now_ms);
        }

        let store = self.store.lock().await;
        let heartbeat_due_at = store.heartbeat_due_at_ms(self.heartbeat_interval_ms, now_ms);
        let cron_due_at = store
            .cron_jobs
            .values()
            .filter(|job| job.enabled && !job.running)
            .filter_map(|job| job.next_run_at_ms)
            .min();
        match (heartbeat_due_at, cron_due_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    async fn process_ready(&self) -> Result<(), String> {
        let now_ms = self.clock.now_ms();
        self.arm_interval_heartbeat(now_ms).await;
        self.enqueue_due_jobs(now_ms).await?;
        self.maybe_enqueue_heartbeat(now_ms).await
    }

    async fn arm_interval_heartbeat(&self, now_ms: u64) {
        let due = {
            let store = self.store.lock().await;
            store
                .heartbeat_due_at_ms(self.heartbeat_interval_ms, now_ms)
                .is_some_and(|due_at| due_at <= now_ms)
        };
        if due {
            self.request_heartbeat_now(Some("interval".to_string()))
                .await;
        }
    }

    async fn enqueue_due_jobs(&self, now_ms: u64) -> Result<(), String> {
        let due_job_ids = {
            let store = self.store.lock().await;
            store.due_job_ids(now_ms)
        };
        for job_id in due_job_ids {
            let job = self
                .mutate_store(|store, _| store.mark_job_started(&job_id, now_ms))
                .await?;
            match job.target {
                CronSessionTarget::Main => {
                    self.mutate_store(|store, _| {
                        store.enqueue_system_event(
                            MAIN_SESSION_KEY,
                            &job.prompt,
                            Some(&format!("cron:{}", job.id)),
                            now_ms,
                        );
                        store.mark_job_finished(
                            &job.id,
                            now_ms,
                            CronJobStatus::QueuedMain,
                            None,
                        )?;
                        Ok(())
                    })
                    .await?;
                    self.request_heartbeat_now(Some(format!("cron:{}", job.id)))
                        .await;
                }
                CronSessionTarget::Isolated => {
                    let prompt = IngressPrompt {
                        source: PromptSource::Automation,
                        session_key: format!("cron:{}", job.id),
                        turn_kind: TurnKind::CronIsolated {
                            job_id: job.id.clone(),
                        },
                        text: job.prompt.clone(),
                        modality: sieve_types::InteractionModality::Text,
                        media_file_id: None,
                    };
                    if let Err(err) = self.prompt_tx.send(prompt) {
                        self.mutate_store(|store, _| {
                            store.mark_job_finished(
                                &job.id,
                                now_ms,
                                CronJobStatus::Failed,
                                Some(format!("failed to enqueue isolated cron prompt: {err}")),
                            )?;
                            Ok(())
                        })
                        .await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn maybe_enqueue_heartbeat(&self, now_ms: u64) -> Result<(), String> {
        if self.main_turns_in_flight.load(Ordering::Relaxed) > 0 {
            return Ok(());
        }
        if self.heartbeat_running.load(Ordering::Relaxed) {
            return Ok(());
        }

        let reason = self.pending_heartbeat_reason.lock().await.clone();
        let Some(reason) = reason else {
            return Ok(());
        };

        let prompt = {
            let store = self.store.lock().await;
            build_heartbeat_prompt(
                &store,
                now_ms,
                Some(&reason),
                self.heartbeat_prompt_override.as_deref(),
                &self.heartbeat_file_path,
            )?
        };

        let Some(prompt) = prompt else {
            self.pending_heartbeat_reason.lock().await.take();
            self.mutate_store(|store, _| {
                store.record_heartbeat_run(now_ms, None);
                Ok(())
            })
            .await?;
            return Ok(());
        };

        self.heartbeat_running.store(true, Ordering::Relaxed);
        self.pending_heartbeat_reason.lock().await.take();
        if let Err(err) = self.prompt_tx.send(IngressPrompt {
            source: PromptSource::Automation,
            session_key: MAIN_SESSION_KEY.to_string(),
            turn_kind: TurnKind::Heartbeat {
                reason: Some(reason),
                queued_event_ids: prompt.queued_event_ids,
            },
            text: prompt.text,
            modality: sieve_types::InteractionModality::Text,
            media_file_id: None,
        }) {
            self.heartbeat_running.store(false, Ordering::Relaxed);
            return Err(format!("failed to enqueue heartbeat prompt: {err}"));
        }
        Ok(())
    }

    async fn mutate_store<T, F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce(&mut AutomationStore, u64) -> Result<T, String>,
    {
        let now_ms = self.clock.now_ms();
        let (result, snapshot) = {
            let mut store = self.store.lock().await;
            let result = f(&mut store, now_ms)?;
            (result, store.clone())
        };
        save_automation_store(&self.store_path, &snapshot)?;
        Ok(result)
    }
}

fn format_cron_job_list(jobs: &[CronJob]) -> String {
    if jobs.is_empty() {
        return "no cron jobs".to_string();
    }
    let mut lines = Vec::with_capacity(jobs.len());
    for job in jobs {
        lines.push(format!(
            "{} {} {} next {}",
            job.id,
            target_label(&job.target),
            job.schedule.describe(),
            render_optional_timestamp(job.next_run_at_ms)
        ));
    }
    lines.join("\n")
}

fn target_label(target: &CronSessionTarget) -> &'static str {
    match target {
        CronSessionTarget::Main => "main",
        CronSessionTarget::Isolated => "isolated",
    }
}

fn render_optional_timestamp(value: Option<u64>) -> String {
    value
        .map(|timestamp_ms| {
            chrono::Utc
                .timestamp_millis_opt(timestamp_ms as i64)
                .single()
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_else(|| timestamp_ms.to_string())
        })
        .unwrap_or_else(|| "disabled".to_string())
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod manager_tests;
