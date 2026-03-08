use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationAction {
    CronList,
    CronAdd,
    CronRemove,
    CronPause,
    CronResume,
}

impl AutomationAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CronList => "cron_list",
            Self::CronAdd => "cron_add",
            Self::CronRemove => "cron_remove",
            Self::CronPause => "cron_pause",
            Self::CronResume => "cron_resume",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTarget {
    Main,
    Isolated,
}

impl AutomationTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isolated => "isolated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDeliveryMode {
    MainSessionMessage,
    IsolatedTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationSchedule {
    After { delay: String },
    At { timestamp: String },
    Every { interval: String },
    Cron { expr: String },
}

impl AutomationSchedule {
    pub const fn kind_str(&self) -> &'static str {
        match self {
            Self::After { .. } => "after",
            Self::At { .. } => "at",
            Self::Every { .. } => "every",
            Self::Cron { .. } => "cron",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRequest {
    pub action: AutomationAction,
    pub target: Option<AutomationTarget>,
    pub schedule: Option<AutomationSchedule>,
    pub prompt: Option<String>,
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TrustedToolEffect {
    CronAdded {
        job_id: String,
        target: AutomationTarget,
        run_at_ms: u64,
        prompt: String,
        delivery_mode: AutomationDeliveryMode,
    },
}
