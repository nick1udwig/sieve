use crate::canonicalize_sink_key;
use crate::config::{DenyRuleDecision, PolicyConfig, ViolationMode};
use crate::normalize::{normalize_capability_scope, normalize_config};
use sieve_types::{
    Action, CapacityType, Capability, CommandKnowledge, Integrity, PolicyDecision,
    PolicyDecisionKind, PrecheckInput, RuntimePolicyContext, SinkChannel, UncertainMode,
    UnknownMode, ValueRef,
};
use thiserror::Error;

pub trait PolicyEngine: Send + Sync {
    fn evaluate_precheck(&self, input: &PrecheckInput) -> PolicyDecision;
}

#[derive(Debug, Error)]
pub enum PolicyConfigError {
    #[error("failed to parse policy TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone)]
pub struct TomlPolicyEngine {
    config: PolicyConfig,
}

impl TomlPolicyEngine {
    pub fn from_toml_str(raw: &str) -> Result<Self, PolicyConfigError> {
        let mut config: PolicyConfig = toml::from_str(raw)?;
        normalize_config(&mut config);
        Ok(Self { config })
    }

    pub fn from_config(config: PolicyConfig) -> Self {
        let mut config = config;
        normalize_config(&mut config);
        Self { config }
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }
}

impl PolicyEngine for TomlPolicyEngine {
    fn evaluate_precheck(&self, input: &PrecheckInput) -> PolicyDecision {
        match input.knowledge {
            CommandKnowledge::Unknown => return decision_for_unknown_mode(input.unknown_mode),
            CommandKnowledge::Uncertain => {
                return decision_for_uncertain_mode(input.uncertain_mode);
            }
            CommandKnowledge::Known => {}
        }

        for segment in &input.command_segments {
            if let Some(rule) = self
                .config
                .deny_rules
                .iter()
                .find(|rule| has_prefix(&segment.argv, &rule.argv_prefix))
            {
                let reason = rule
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("command blocked by deny rule {}", rule.id));
                return match rule.decision {
                    DenyRuleDecision::Deny => PolicyDecision {
                        kind: PolicyDecisionKind::Deny,
                        reason,
                        blocked_rule_id: Some(rule.id.clone()),
                    },
                    DenyRuleDecision::DenyWithApproval => PolicyDecision {
                        kind: PolicyDecisionKind::DenyWithApproval,
                        reason,
                        blocked_rule_id: Some(rule.id.clone()),
                    },
                };
            }
        }

        let Some(summary) = &input.summary else {
            return PolicyDecision {
                kind: PolicyDecisionKind::Deny,
                reason: "missing command summary for known command".to_string(),
                blocked_rule_id: Some("known-missing-summary".to_string()),
            };
        };

        if !summary.unsupported_flags.is_empty() {
            return decision_for_unknown_mode(input.unknown_mode);
        }

        if self.config.options.require_trusted_control_for_mutating
            && !self.control_is_trusted(input)
            && has_consequential_capability(&summary.required_capabilities)
        {
            return PolicyDecision {
                kind: PolicyDecisionKind::DenyWithApproval,
                reason: "untrusted control context for consequential action".to_string(),
                blocked_rule_id: Some("integrity-untrusted-control".to_string()),
            };
        }

        for cap in &summary.required_capabilities {
            if !self.capability_allowed(cap, &input.cwd) {
                return self.policy_violation_decision(
                    format!("missing capability {}", capability_key(cap, &input.cwd)),
                    "missing-capability",
                );
            }
        }

        for check in &summary.sink_checks {
            let sink_canonical =
                canonicalize_sink_key(&check.sink.0).unwrap_or_else(|_| check.sink.0.clone());
            for value_ref in &check.value_refs {
                if self.runtime_value_requires_typed_extraction(value_ref, &input.runtime_context) {
                    return self.policy_violation_decision(
                        format!(
                            "trusted_string value {} requires typed extraction before sink flow",
                            value_ref.0
                        ),
                        "sink-capacity-denied",
                    );
                }
                if !self.value_allows_sink(
                    value_ref,
                    &sink_canonical,
                    check.channel,
                    &input.runtime_context,
                ) {
                    return self.policy_violation_decision(
                        format!(
                            "value {} cannot flow to sink {} channel {} for arg {}",
                            value_ref.0,
                            sink_canonical,
                            format!("{:?}", check.channel).to_lowercase(),
                            check.argument_name
                        ),
                        "sink-flow-denied",
                    );
                }
            }
        }

        PolicyDecision {
            kind: PolicyDecisionKind::Allow,
            reason: "policy checks passed".to_string(),
            blocked_rule_id: None,
        }
    }
}

fn decision_for_unknown_mode(mode: UnknownMode) -> PolicyDecision {
    match mode {
        UnknownMode::Deny => PolicyDecision {
            kind: PolicyDecisionKind::Deny,
            reason: "unknown command denied by mode".to_string(),
            blocked_rule_id: Some("unknown-mode".to_string()),
        },
        UnknownMode::Ask => PolicyDecision {
            kind: PolicyDecisionKind::DenyWithApproval,
            reason: "unknown command requires approval".to_string(),
            blocked_rule_id: Some("unknown-mode".to_string()),
        },
        UnknownMode::Accept => PolicyDecision {
            kind: PolicyDecisionKind::Allow,
            reason: "unknown command accepted by mode".to_string(),
            blocked_rule_id: None,
        },
    }
}

fn decision_for_uncertain_mode(mode: UncertainMode) -> PolicyDecision {
    match mode {
        UncertainMode::Deny => PolicyDecision {
            kind: PolicyDecisionKind::Deny,
            reason: "uncertain command denied by mode".to_string(),
            blocked_rule_id: Some("uncertain-mode".to_string()),
        },
        UncertainMode::Ask => PolicyDecision {
            kind: PolicyDecisionKind::DenyWithApproval,
            reason: "uncertain command requires approval".to_string(),
            blocked_rule_id: Some("uncertain-mode".to_string()),
        },
        UncertainMode::Accept => PolicyDecision {
            kind: PolicyDecisionKind::Allow,
            reason: "uncertain command accepted by mode".to_string(),
            blocked_rule_id: None,
        },
    }
}

fn has_prefix(argv: &[String], prefix: &[String]) -> bool {
    !prefix.is_empty()
        && argv.len() >= prefix.len()
        && argv.iter().zip(prefix.iter()).all(|(a, b)| a == b)
}

fn has_consequential_capability(required: &[Capability]) -> bool {
    required.iter().any(|cap| cap.action != Action::Read)
}

fn capability_key(cap: &Capability, cwd: &str) -> String {
    format!(
        "{:?}:{:?}:{}",
        cap.resource,
        cap.action,
        normalize_capability_scope(cap.resource, &cap.scope, cwd)
    )
}

impl TomlPolicyEngine {
    fn capability_allowed(&self, cap: &Capability, cwd: &str) -> bool {
        let scope = normalize_capability_scope(cap.resource, &cap.scope, cwd);

        self.config.allow_capabilities.iter().any(|candidate| {
            let candidate_scope =
                normalize_capability_scope(candidate.resource, &candidate.scope, cwd);
            candidate.resource == cap.resource
                && candidate.action == cap.action
                && candidate_scope == scope
        })
    }

    fn control_is_trusted(&self, input: &PrecheckInput) -> bool {
        self.config.options.trusted_control
            && input.runtime_context.control.integrity == Integrity::Trusted
    }

    fn value_allows_sink(
        &self,
        value_ref: &ValueRef,
        sink: &str,
        channel: SinkChannel,
        runtime_context: &RuntimePolicyContext,
    ) -> bool {
        self.runtime_value_allows_sink(value_ref, sink, channel, runtime_context)
            || self.config_value_allows_sink(value_ref, sink, channel)
    }

    fn runtime_value_allows_sink(
        &self,
        value_ref: &ValueRef,
        sink: &str,
        channel: SinkChannel,
        runtime_context: &RuntimePolicyContext,
    ) -> bool {
        runtime_context
            .sink_permissions
            .allowed_sinks_by_value
            .get(value_ref)
            .is_some_and(|sinks| {
                sinks.iter().any(|allowed_sink| {
                    allowed_sink.channel == channel
                        && canonicalize_sink_key(&allowed_sink.sink.0)
                            .unwrap_or_else(|_| allowed_sink.sink.0.clone())
                            == sink
                })
            })
            || runtime_context
                .sink_permissions
                .released_sinks_by_source_value
                .get(value_ref)
                .is_some_and(|sinks| {
                    sinks.iter().any(|allowed_sink| {
                        allowed_sink.channel == channel
                            && canonicalize_sink_key(&allowed_sink.sink.0)
                                .unwrap_or_else(|_| allowed_sink.sink.0.clone())
                                == sink
                    })
                })
    }

    fn config_value_allows_sink(
        &self,
        value_ref: &ValueRef,
        sink: &str,
        channel: SinkChannel,
    ) -> bool {
        if channel != SinkChannel::Body {
            return false;
        }
        self.config
            .value_sinks
            .get(&value_ref.0)
            .is_some_and(|sinks| sinks.contains(sink))
    }

    fn runtime_value_requires_typed_extraction(
        &self,
        value_ref: &ValueRef,
        runtime_context: &RuntimePolicyContext,
    ) -> bool {
        runtime_context
            .sink_permissions
            .capacity_type_by_value
            .get(value_ref)
            .is_some_and(|capacity_type| *capacity_type == CapacityType::TrustedString)
    }

    fn policy_violation_decision(&self, reason: String, rule_id: &str) -> PolicyDecision {
        match self.config.options.violation_mode {
            ViolationMode::Deny => PolicyDecision {
                kind: PolicyDecisionKind::Deny,
                reason,
                blocked_rule_id: Some(rule_id.to_string()),
            },
            ViolationMode::Ask => PolicyDecision {
                kind: PolicyDecisionKind::DenyWithApproval,
                reason,
                blocked_rule_id: Some(rule_id.to_string()),
            },
        }
    }
}
