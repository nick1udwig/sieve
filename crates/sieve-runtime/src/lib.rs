#![forbid(unsafe_code)]

mod approval_allowance;
mod approval_bus;
mod approval_tools;
mod automation;
mod browser_session_summary;
mod browser_sessions;
mod codex;
mod event_log;
mod mainline;
mod orchestrator;
mod planner_turn;
mod shell_gate;
mod value_state;

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_command_summaries::{CommandSummarizer, SummaryOutcome};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_llm::PlannerModel;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_policy::PolicyEngine;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_quarantine::{QuarantineRunError, QuarantineRunner};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_shell::{ShellAnalysisError, ShellAnalyzer};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use sieve_types::{
    ApprovalAction, ApprovalRequestId, ApprovalRequestedEvent, CodexExecRequest, CodexSandboxMode,
    CodexSessionRequest, CodexTurnResult, CodexTurnStatus, CommandSegment, DeclassifyRequest,
    EndorseRequest, PlannerCodexSession, PolicyDecisionKind, PrecheckInput, QuarantineReport,
    QuarantineRunRequest, RunId, RuntimeEvent, UncertainMode, UnknownMode,
};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use std::sync::Arc;

pub use approval_bus::{ApprovalBus, ApprovalBusError, InProcessApprovalBus};
pub use automation::{AutomationTool, AutomationToolResult};
pub use codex::{CodexTool, CodexToolResult};
pub use event_log::{EventLogError, JsonlRuntimeEventLog, RuntimeEventLog};
pub use mainline::{
    BashMainlineRunner, MainlineArtifact, MainlineArtifactKind, MainlineRunError,
    MainlineRunReport, MainlineRunRequest, MainlineRunner,
};
pub use orchestrator::{Clock, RuntimeDeps, RuntimeError, RuntimeOrchestrator, SystemClock};
pub use planner_turn::{PlannerRunRequest, PlannerRunResult, PlannerToolResult};
pub use shell_gate::{RuntimeDisposition, ShellRunRequest};
pub use value_state::ValueStateError;

#[cfg(test)]
mod tests;
