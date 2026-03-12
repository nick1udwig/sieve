use super::{base_input, engine_with_default_policy};
use crate::{PolicyEngine, TomlPolicyEngine};
use sieve_types::{
    Action, Capability, CommandKnowledge, CommandSegment, CommandSummary, Integrity,
    PolicyDecisionKind, Resource, SinkChannel, SinkCheck, SinkKey, SinkPermission, UncertainMode,
    UnknownMode, ValueRef,
};
use std::collections::BTreeSet;

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
            channel: SinkChannel::Body,
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
fn denies_mutating_action_when_runtime_control_untrusted() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

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
    input.runtime_context.control.integrity = Integrity::Untrusted;

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::DenyWithApproval);
    assert_eq!(
        decision.blocked_rule_id.as_deref(),
        Some("integrity-untrusted-control")
    );
}

#[test]
fn allows_sink_when_runtime_context_permits_value_flow() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

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
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef("body_ref".to_string())],
        }],
        unsupported_flags: vec![],
    });
    input
        .runtime_context
        .sink_permissions
        .allowed_sinks_by_value
        .insert(
            ValueRef("body_ref".to_string()),
            BTreeSet::from([SinkPermission {
                sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                channel: SinkChannel::Body,
            }]),
        );

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Allow);
}

#[test]
fn allows_sink_when_runtime_context_has_release_grant_for_source_value() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

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
            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef("source_ref".to_string())],
        }],
        unsupported_flags: vec![],
    });
    input
        .runtime_context
        .sink_permissions
        .released_sinks_by_source_value
        .insert(
            ValueRef("source_ref".to_string()),
            BTreeSet::from([SinkPermission {
                sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                channel: SinkChannel::Body,
            }]),
        );

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Allow);
}

#[test]
fn denies_sink_when_runtime_value_is_trusted_string_even_with_release_grant() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

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
            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef("source_ref".to_string())],
        }],
        unsupported_flags: vec![],
    });
    input
        .runtime_context
        .sink_permissions
        .released_sinks_by_source_value
        .insert(
            ValueRef("source_ref".to_string()),
            BTreeSet::from([SinkPermission {
                sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                channel: SinkChannel::Body,
            }]),
        );
    input
        .runtime_context
        .sink_permissions
        .capacity_type_by_value
        .insert(
            ValueRef("source_ref".to_string()),
            sieve_types::CapacityType::TrustedString,
        );

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Deny);
    assert_eq!(
        decision.blocked_rule_id.as_deref(),
        Some("sink-capacity-denied")
    );
    assert!(decision.reason.contains("typed extraction"));
}

#[test]
fn allows_sink_when_toml_value_sinks_permits_value_flow() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

[value_sinks]
body_ref = ["https://api.example.com/v1/upload"]

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
            channel: SinkChannel::Body,
            value_refs: vec![ValueRef("body_ref".to_string())],
        }],
        unsupported_flags: vec![],
    });

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Allow);
}

#[test]
fn denies_sink_when_same_destination_permission_has_wrong_channel() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "net"
action = "write"
scope = "https://api.example.com/v1/upload"

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
        "-H".to_string(),
        "Authorization: secret".to_string(),
    ];
    input.summary = Some(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: "https://api.example.com/v1/upload".to_string(),
        }],
        sink_checks: vec![SinkCheck {
            argument_name: "-H".to_string(),
            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: SinkChannel::Header,
            value_refs: vec![ValueRef("header_ref".to_string())],
        }],
        unsupported_flags: vec![],
    });
    input
        .runtime_context
        .sink_permissions
        .allowed_sinks_by_value
        .insert(
            ValueRef("header_ref".to_string()),
            BTreeSet::from([SinkPermission {
                sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                channel: SinkChannel::Body,
            }]),
        );

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
fn default_violation_mode_is_ask() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[options]
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
fn allows_relative_fs_scope_when_absolute_capability_present() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[[allow_capabilities]]
resource = "fs"
action = "write"
scope = "/tmp/workspace/dst.txt"

[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
    )
    .expect("policy parse");

    let mut input = base_input();
    input.cwd = "/tmp/workspace".to_string();
    input.summary = Some(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "./out/../dst.txt".to_string(),
        }],
        sink_checks: vec![],
        unsupported_flags: vec![],
    });

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Allow);
}

#[test]
fn missing_fs_capability_reason_uses_normalized_scope() {
    let engine = TomlPolicyEngine::from_toml_str(
        r#"
[options]
violation_mode = "deny"
trusted_control = true
require_trusted_control_for_mutating = true
"#,
    )
    .expect("policy parse");

    let mut input = base_input();
    input.cwd = "/tmp/workspace".to_string();
    input.summary = Some(CommandSummary {
        required_capabilities: vec![Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: "dst.txt".to_string(),
        }],
        sink_checks: vec![],
        unsupported_flags: vec![],
    });

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Deny);
    assert!(decision.reason.contains("Fs:Write:/tmp/workspace/dst.txt"));
}
