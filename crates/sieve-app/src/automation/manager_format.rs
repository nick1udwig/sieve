use super::types::render_timestamp_ms;
use super::{CronJob, CronSessionTarget};
use sieve_types::{AutomationDeliveryMode, AutomationTarget, TrustedToolEffect};

pub(crate) fn format_cron_job_list(jobs: &[CronJob]) -> String {
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

pub(crate) fn target_label(target: &CronSessionTarget) -> &'static str {
    match target {
        CronSessionTarget::Main => "main",
        CronSessionTarget::Isolated => "isolated",
    }
}

pub(crate) fn trusted_effect_for_added_job(job: &CronJob) -> TrustedToolEffect {
    TrustedToolEffect::CronAdded {
        job_id: job.id.clone(),
        target: match job.target {
            CronSessionTarget::Main => AutomationTarget::Main,
            CronSessionTarget::Isolated => AutomationTarget::Isolated,
        },
        run_at_ms: job.next_run_at_ms.unwrap_or(job.created_at_ms),
        prompt: job.prompt.clone(),
        delivery_mode: match job.target {
            CronSessionTarget::Main => AutomationDeliveryMode::MainSessionMessage,
            CronSessionTarget::Isolated => AutomationDeliveryMode::IsolatedTurn,
        },
    }
}

pub(crate) fn render_optional_timestamp(value: Option<u64>) -> String {
    value
        .map(render_timestamp_ms)
        .unwrap_or_else(|| "disabled".to_string())
}
