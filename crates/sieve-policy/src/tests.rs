use super::*;
use sieve_types::{
    Action, Capability, CommandKnowledge, CommandSegment, CommandSummary, ControlContext,
    Integrity, PolicyDecisionKind, PrecheckInput, Resource, RunId, RuntimePolicyContext, SinkCheck,
    SinkKey, SinkPermissionContext, UncertainMode, UnknownMode, ValueRef,
};
use std::collections::BTreeSet;

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
            BTreeSet::from([SinkKey("https://api.example.com/v1/upload".to_string())]),
        );

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Allow);
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
            value_refs: vec![ValueRef("body_ref".to_string())],
        }],
        unsupported_flags: vec![],
    });

    let decision = engine.evaluate_precheck(&input);
    assert_eq!(decision.kind, PolicyDecisionKind::Allow);
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

#[test]
fn canonicalizes_url_sink_keys() {
    let sink = canonicalize_sink_key("HTTPS://Api.Example.Com:443/v1/../v1/%7euser?q=1#frag")
        .expect("canonicalization");
    assert_eq!(sink, "https://api.example.com/v1/~user");

    let sink2 = canonicalize_sink_key("http://EXAMPLE.com:80").expect("canonicalization");
    assert_eq!(sink2, "http://example.com/");
}

#[test]
fn canonicalizes_net_origin_scopes() {
    assert_eq!(
        canonicalize_net_origin_scope("HTTPS://Api.Example.Com:443/v1/path?q=1#frag"),
        Some("https://api.example.com".to_string())
    );
    assert_eq!(
        canonicalize_net_origin_scope("http://EXAMPLE.com:8080/path"),
        Some("http://example.com:8080".to_string())
    );
    assert_eq!(canonicalize_net_origin_scope("not-a-url"), None);
}
