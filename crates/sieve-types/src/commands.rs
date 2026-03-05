use crate::{Capability, RunId, RuntimePolicyContext, SinkKey, ValueRef};
use serde::{Deserialize, Serialize};

/// Policy mode for commands parsed but not summarized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownMode {
    Ask,
    Accept,
    Deny,
}

/// Policy mode for unsupported shell constructs/parser uncertainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertainMode {
    Ask,
    Accept,
    Deny,
}

/// Supported command composition operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionOperator {
    Sequence,
    And,
    Or,
    Pipe,
}

/// One parsed command segment in a composed command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSegment {
    pub argv: Vec<String>,
    pub operator_before: Option<CompositionOperator>,
}

/// Classifier output for command understanding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKnowledge {
    Known,
    Unknown,
    Uncertain,
}

/// Confidentiality check for one sink-bearing argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkCheck {
    pub argument_name: String,
    pub sink: SinkKey,
    pub value_refs: Vec<ValueRef>,
}

/// Summary output used by precheck evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSummary {
    pub required_capabilities: Vec<Capability>,
    pub sink_checks: Vec<SinkCheck>,
    pub unsupported_flags: Vec<String>,
}

/// Input to policy precheck evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecheckInput {
    pub run_id: RunId,
    pub cwd: String,
    pub command_segments: Vec<CommandSegment>,
    pub knowledge: CommandKnowledge,
    pub summary: Option<CommandSummary>,
    pub runtime_context: RuntimePolicyContext,
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}
