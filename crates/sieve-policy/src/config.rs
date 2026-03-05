use serde::Deserialize;
use sieve_types::Action;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationMode {
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PolicyConfig {
    #[serde(default)]
    pub deny_rules: Vec<DenyRule>,
    #[serde(default)]
    pub allow_capabilities: Vec<CapabilityPolicy>,
    #[serde(default)]
    pub value_sinks: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub options: PolicyOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PolicyOptions {
    #[serde(default = "default_violation_mode")]
    pub violation_mode: ViolationMode,
    #[serde(default = "default_true")]
    pub require_trusted_control_for_mutating: bool,
    #[serde(default = "default_true")]
    pub trusted_control: bool,
}

impl Default for PolicyOptions {
    fn default() -> Self {
        Self {
            violation_mode: default_violation_mode(),
            require_trusted_control_for_mutating: true,
            trusted_control: true,
        }
    }
}

const fn default_true() -> bool {
    true
}

const fn default_violation_mode() -> ViolationMode {
    ViolationMode::Ask
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DenyRule {
    pub id: String,
    pub argv_prefix: Vec<String>,
    #[serde(default = "default_deny_rule_decision")]
    pub decision: DenyRuleDecision,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DenyRuleDecision {
    Deny,
    DenyWithApproval,
}

const fn default_deny_rule_decision() -> DenyRuleDecision {
    DenyRuleDecision::Deny
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CapabilityPolicy {
    pub resource: sieve_types::Resource,
    pub action: Action,
    pub scope: String,
}
