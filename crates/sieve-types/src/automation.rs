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
pub enum AutomationScheduleKind {
    Every,
    At,
    Cron,
}

impl AutomationScheduleKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Every => "every",
            Self::At => "at",
            Self::Cron => "cron",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRequest {
    pub action: AutomationAction,
    pub target: Option<AutomationTarget>,
    pub schedule_kind: Option<AutomationScheduleKind>,
    pub schedule: Option<String>,
    pub prompt: Option<String>,
    pub job_id: Option<String>,
}
