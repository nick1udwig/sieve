#![forbid(unsafe_code)]

use serde::Deserialize;
use sieve_types::{
    Action, Capability, CommandKnowledge, PolicyDecision, PolicyDecisionKind, PrecheckInput,
    SinkKey, UncertainMode, UnknownMode, ValueRef,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use url::Url;

pub trait PolicyEngine: Send + Sync {
    fn evaluate_precheck(&self, input: &PrecheckInput) -> PolicyDecision;
}

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
    ViolationMode::Deny
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
                return decision_for_uncertain_mode(input.uncertain_mode)
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
            && !self.config.options.trusted_control
            && has_consequential_capability(&summary.required_capabilities)
        {
            return PolicyDecision {
                kind: PolicyDecisionKind::DenyWithApproval,
                reason: "untrusted control context for consequential action".to_string(),
                blocked_rule_id: Some("integrity-untrusted-control".to_string()),
            };
        }

        for cap in &summary.required_capabilities {
            if !self.capability_allowed(cap) {
                return self.policy_violation_decision(
                    format!("missing capability {}", capability_key(cap)),
                    "missing-capability",
                );
            }
        }

        for check in &summary.sink_checks {
            let sink_canonical =
                canonicalize_sink_key(&check.sink.0).unwrap_or_else(|_| check.sink.0.clone());
            for value_ref in &check.value_refs {
                if !self.value_allows_sink(value_ref, &sink_canonical) {
                    return self.policy_violation_decision(
                        format!(
                            "value {} cannot flow to sink {} for arg {}",
                            value_ref.0, sink_canonical, check.argument_name
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

fn capability_key(cap: &Capability) -> String {
    format!("{:?}:{:?}:{}", cap.resource, cap.action, cap.scope)
}

fn normalize_config(config: &mut PolicyConfig) {
    for cap in &mut config.allow_capabilities {
        if cap.resource == sieve_types::Resource::Net {
            cap.scope = canonicalize_sink_key(&cap.scope).unwrap_or_else(|_| cap.scope.clone());
        }
    }

    for sinks in config.value_sinks.values_mut() {
        let normalized: BTreeSet<String> = sinks
            .iter()
            .map(|sink| canonicalize_sink_key(sink).unwrap_or_else(|_| sink.clone()))
            .collect();
        *sinks = normalized;
    }
}

impl TomlPolicyEngine {
    fn capability_allowed(&self, cap: &Capability) -> bool {
        let scope = if cap.resource == sieve_types::Resource::Net {
            canonicalize_sink_key(&cap.scope).unwrap_or_else(|_| cap.scope.clone())
        } else {
            cap.scope.clone()
        };

        self.config.allow_capabilities.iter().any(|candidate| {
            candidate.resource == cap.resource
                && candidate.action == cap.action
                && candidate.scope == scope
        })
    }

    fn value_allows_sink(&self, value_ref: &ValueRef, sink: &str) -> bool {
        self.config
            .value_sinks
            .get(&value_ref.0)
            .is_some_and(|sinks| sinks.contains(sink))
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

#[derive(Debug, Error)]
pub enum SinkCanonicalizationError {
    #[error("invalid URL sink: {0}")]
    Invalid(#[from] url::ParseError),
}

pub fn canonicalize_sink_key(raw: &str) -> Result<String, SinkCanonicalizationError> {
    let mut url = Url::parse(raw)?;
    url.set_query(None);
    url.set_fragment(None);

    // Trigger serde_url normalization pass for path and percent-encoding semantics.
    let normalized = Url::parse(url.as_str())?;
    let mut out = normalized;

    let is_http_default = out.scheme() == "http" && out.port() == Some(80);
    let is_https_default = out.scheme() == "https" && out.port() == Some(443);
    if is_http_default || is_https_default {
        let _ = out.set_port(None);
    }

    let host = out.host_str().unwrap_or_default();
    let port_part = out.port().map(|p| format!(":{p}")).unwrap_or_default();
    let raw_path = if out.path().is_empty() {
        "/"
    } else {
        out.path()
    };
    let path = normalize_percent_encoding(raw_path);

    Ok(format!("{}://{}{}{}", out.scheme(), host, port_part, path))
}

fn normalize_percent_encoding(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1];
            let h2 = bytes[i + 2];
            if let (Some(n1), Some(n2)) = (hex_val(h1), hex_val(h2)) {
                let value = (n1 << 4) | n2;
                if is_unreserved(value) {
                    out.push(value as char);
                } else {
                    out.push('%');
                    out.push(hex_upper(n1));
                    out.push(hex_upper(n2));
                }
                i += 3;
                continue;
            }
        }

        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_upper(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => '0',
    }
}

fn is_unreserved(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~')
}

pub fn canonicalize_sink_set(sinks: &BTreeSet<SinkKey>) -> BTreeSet<SinkKey> {
    sinks
        .iter()
        .map(|sink| {
            let normalized = canonicalize_sink_key(&sink.0).unwrap_or_else(|_| sink.0.clone());
            SinkKey(normalized)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::{
        Action, Capability, CommandKnowledge, CommandSegment, CommandSummary, ControlContext,
        Integrity, PolicyDecisionKind, PrecheckInput, Resource, RunId, RuntimePolicyContext,
        SinkCheck, SinkKey, SinkPermissionContext, UncertainMode, UnknownMode, ValueRef,
    };

    fn engine_with_default_policy() -> TomlPolicyEngine {
        TomlPolicyEngine::from_toml_str(
            r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"
reason = "rm -rf requires approval"

[options]
violation_mode = "deny"
require_trusted_control_for_mutating = true
trusted_control = true
"#,
        )
        .expect("policy parse")
    }

    fn base_input() -> PrecheckInput {
        PrecheckInput {
            run_id: RunId("r1".to_string()),
            cwd: "/tmp".to_string(),
            command_segments: vec![CommandSegment {
                argv: vec!["echo".to_string(), "ok".to_string()],
                operator_before: None,
            }],
            knowledge: CommandKnowledge::Known,
            summary: Some(CommandSummary {
                required_capabilities: vec![],
                sink_checks: vec![],
                unsupported_flags: vec![],
            }),
            runtime_context: RuntimePolicyContext {
                control: ControlContext {
                    integrity: Integrity::Trusted,
                    value_refs: BTreeSet::new(),
                    endorsed_by: None,
                },
                sink_permissions: SinkPermissionContext::default(),
            },
            unknown_mode: UnknownMode::Deny,
            uncertain_mode: UncertainMode::Deny,
        }
    }

    #[test]
    fn blocks_rm_rf_with_approval() {
        let engine = engine_with_default_policy();
        let mut input = base_input();
        input.command_segments[0].argv = vec!["rm".to_string(), "-rf".to_string(), "/".to_string()];

        let decision = engine.evaluate_precheck(&input);
        assert_eq!(decision.kind, PolicyDecisionKind::DenyWithApproval);
        assert_eq!(decision.blocked_rule_id.as_deref(), Some("deny-rm-rf"));
    }

    #[test]
    fn denies_post_missing_capability() {
        let engine = engine_with_default_policy();
        let mut input = base_input();
        input.command_segments[0].argv = vec![
            "curl".to_string(),
            "-X".to_string(),
            "POST".to_string(),
            "https://api.example.com/v1/upload".to_string(),
        ];
        input.summary = Some(CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1/upload".to_string(),
            }],
            sink_checks: vec![],
            unsupported_flags: vec![],
        });

        let decision = engine.evaluate_precheck(&input);
        assert_eq!(decision.kind, PolicyDecisionKind::Deny);
        assert_eq!(
            decision.blocked_rule_id.as_deref(),
            Some("missing-capability")
        );
    }

    #[test]
    fn denies_payload_sink_violation() {
        let engine = TomlPolicyEngine::from_toml_str(
            r#"
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval"

[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

[value_sinks]
body_ref = ["https://api.example.com/v1/other"]

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        )
        .expect("policy parse");

        let mut input = base_input();
        input.command_segments[0].argv = vec![
            "curl".to_string(),
            "-X".to_string(),
            "POST".to_string(),
            "https://api.example.com/v1/upload".to_string(),
            "-d".to_string(),
            "{}".to_string(),
        ];
        input.summary = Some(CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/v1/upload".to_string(),
            }],
            sink_checks: vec![SinkCheck {
                argument_name: "body".to_string(),
                sink: SinkKey("https://api.example.com/v1/upload?token=abc".to_string()),
                value_refs: vec![ValueRef("body_ref".to_string())],
            }],
            unsupported_flags: vec![],
        });

        let decision = engine.evaluate_precheck(&input);
        assert_eq!(decision.kind, PolicyDecisionKind::Deny);
        assert_eq!(
            decision.blocked_rule_id.as_deref(),
            Some("sink-flow-denied")
        );
    }

    #[test]
    fn unknown_and_uncertain_modes_follow_policy() {
        let engine = engine_with_default_policy();

        let mut unknown = base_input();
        unknown.knowledge = CommandKnowledge::Unknown;
        unknown.unknown_mode = UnknownMode::Ask;
        assert_eq!(
            engine.evaluate_precheck(&unknown).kind,
            PolicyDecisionKind::DenyWithApproval
        );

        unknown.unknown_mode = UnknownMode::Accept;
        assert_eq!(
            engine.evaluate_precheck(&unknown).kind,
            PolicyDecisionKind::Allow
        );

        unknown.unknown_mode = UnknownMode::Deny;
        assert_eq!(
            engine.evaluate_precheck(&unknown).kind,
            PolicyDecisionKind::Deny
        );

        let mut uncertain = base_input();
        uncertain.knowledge = CommandKnowledge::Uncertain;
        uncertain.uncertain_mode = UncertainMode::Ask;
        assert_eq!(
            engine.evaluate_precheck(&uncertain).kind,
            PolicyDecisionKind::DenyWithApproval
        );

        uncertain.uncertain_mode = UncertainMode::Accept;
        assert_eq!(
            engine.evaluate_precheck(&uncertain).kind,
            PolicyDecisionKind::Allow
        );

        uncertain.uncertain_mode = UncertainMode::Deny;
        assert_eq!(
            engine.evaluate_precheck(&uncertain).kind,
            PolicyDecisionKind::Deny
        );
    }

    #[test]
    fn composed_commands_are_all_or_nothing() {
        let engine = engine_with_default_policy();
        let mut input = base_input();
        input.command_segments = vec![
            CommandSegment {
                argv: vec!["echo".to_string(), "ok".to_string()],
                operator_before: None,
            },
            CommandSegment {
                argv: vec!["rm".to_string(), "-rf".to_string(), "/tmp/x".to_string()],
                operator_before: Some(sieve_types::CompositionOperator::And),
            },
        ];

        let decision = engine.evaluate_precheck(&input);
        assert_eq!(decision.kind, PolicyDecisionKind::DenyWithApproval);
        assert_eq!(decision.blocked_rule_id.as_deref(), Some("deny-rm-rf"));
    }

    #[test]
    fn violation_mode_ask_returns_deny_with_approval() {
        let engine = TomlPolicyEngine::from_toml_str(
            r#"
[options]
violation_mode = "ask"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
        )
        .expect("policy parse");

        let mut input = base_input();
        input.summary = Some(CommandSummary {
            required_capabilities: vec![Capability {
                resource: Resource::Net,
                action: Action::Write,
                scope: "https://api.example.com/upload".to_string(),
            }],
            sink_checks: vec![],
            unsupported_flags: vec![],
        });

        let decision = engine.evaluate_precheck(&input);
        assert_eq!(decision.kind, PolicyDecisionKind::DenyWithApproval);
        assert_eq!(
            decision.blocked_rule_id.as_deref(),
            Some("missing-capability")
        );
    }

    #[test]
    fn canonicalizes_url_sink_keys() {
        let sink = canonicalize_sink_key("HTTPS://Api.Example.Com:443/v1/../v1/%7euser?q=1#frag")
            .expect("canonicalization");
        assert_eq!(sink, "https://api.example.com/v1/~user");

        let sink2 = canonicalize_sink_key("http://EXAMPLE.com:80").expect("canonicalization");
        assert_eq!(sink2, "http://example.com/");
    }
}
