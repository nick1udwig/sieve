use crate::TomlPolicyEngine;
use sieve_types::{
    CommandKnowledge, CommandSegment, CommandSummary, ControlContext, Integrity, PrecheckInput,
    RunId, RuntimePolicyContext, SinkPermissionContext, UncertainMode, UnknownMode,
};
use std::collections::BTreeSet;

mod engine;
mod normalize;

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
