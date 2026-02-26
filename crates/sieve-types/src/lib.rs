#![forbid(unsafe_code)]

mod contract_freeze_v1;

pub use contract_freeze_v1::*;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Unix epoch milliseconds.
pub type UnixMillis = u64;

/// Stable identifier for one command run attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// Stable identifier for a labeled runtime value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ValueRef(pub String);

/// Stable identifier for one approval prompt lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApprovalRequestId(pub String);

/// Canonical sink key (`scheme://host[:port]/path`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SinkKey(pub String);

/// Capability resource dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Fs,
    Net,
    Proc,
    Env,
    Ipc,
}

/// Capability action dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Read,
    Write,
    Append,
    Exec,
    Connect,
}

/// Capability tuple `(resource, action, scope)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub resource: Resource,
    pub action: Action,
    pub scope: String,
}

/// Value integrity label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Integrity {
    Trusted,
    Untrusted,
}

/// Planner-visible capacity type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityType {
    Bool,
    Int,
    Float,
    Enum,
    TrustedString,
}

/// Provenance source for labeled values.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    User,
    Config,
    Assistant,
    TrustedToolSource,
    Tool {
        tool_name: String,
        inner_sources: BTreeSet<String>,
    },
    Quarantine {
        run_id: RunId,
    },
}

/// Runtime label carried by values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueLabel {
    pub integrity: Integrity,
    pub provenance: BTreeSet<Source>,
    pub allowed_sinks: BTreeSet<SinkKey>,
    pub capacity_type: CapacityType,
}

/// Typed value that can flow from quarantine to planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum TypedValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Enum { registry: String, variant: String },
}

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
    pub unknown_mode: UnknownMode,
    pub uncertain_mode: UncertainMode,
}

/// High-level policy decision class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    Allow,
    DenyWithApproval,
    Deny,
}

/// Policy decision with human-readable reasoning metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub kind: PolicyDecisionKind,
    pub reason: String,
    pub blocked_rule_id: Option<String>,
}

/// User decision for an approval prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAction {
    ApproveOnce,
    Deny,
}

/// Event emitted when runtime asks user for command approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestedEvent {
    pub schema_version: u16,
    pub request_id: ApprovalRequestId,
    pub run_id: RunId,
    pub command_segments: Vec<CommandSegment>,
    pub inferred_capabilities: Vec<Capability>,
    pub blocked_rule_id: String,
    pub reason: String,
    pub created_at_ms: UnixMillis,
}

/// Event emitted when user resolves an approval prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResolvedEvent {
    pub schema_version: u16,
    pub request_id: ApprovalRequestId,
    pub run_id: RunId,
    pub action: ApprovalAction,
    pub created_at_ms: UnixMillis,
}

/// Event emitted after policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEvaluatedEvent {
    pub schema_version: u16,
    pub run_id: RunId,
    pub decision: PolicyDecision,
    pub inferred_capabilities: Vec<Capability>,
    pub trace_path: Option<String>,
    pub created_at_ms: UnixMillis,
}

/// Request to execute a composed command in quarantine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineRunRequest {
    pub run_id: RunId,
    pub cwd: String,
    pub command_segments: Vec<CommandSegment>,
}

/// Quarantine execution report payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineReport {
    pub run_id: RunId,
    pub trace_path: String,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub attempted_capabilities: Vec<Capability>,
    pub exit_code: Option<i32>,
}

/// Event emitted after quarantine run completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineCompletedEvent {
    pub schema_version: u16,
    pub run_id: RunId,
    pub report: QuarantineReport,
    pub created_at_ms: UnixMillis,
}

/// Union of runtime audit events written to JSONL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RuntimeEvent {
    ApprovalRequested(ApprovalRequestedEvent),
    ApprovalResolved(ApprovalResolvedEvent),
    PolicyEvaluated(PolicyEvaluatedEvent),
    QuarantineCompleted(QuarantineCompletedEvent),
}

/// Request payload for explicit `endorse` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndorseRequest {
    pub value_ref: ValueRef,
    pub target_integrity: Integrity,
    pub reason: Option<String>,
}

/// Success response payload for explicit `endorse` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndorseResponse {
    pub value_ref: ValueRef,
    pub integrity: Integrity,
}

/// Request payload for explicit `declassify` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclassifyRequest {
    pub value_ref: ValueRef,
    pub sink: SinkKey,
    pub reason: Option<String>,
}

/// Success response payload for explicit `declassify` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclassifyResponse {
    pub value_ref: ValueRef,
    pub allowed_sinks_added: Vec<SinkKey>,
}

/// Supported LLM provider enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    OpenAi,
}

/// Configuration for one planner model endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmModelConfig {
    pub provider: LlmProvider,
    pub model: String,
    pub api_base: Option<String>,
}

/// Planner invocation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerTurnInput {
    pub run_id: RunId,
    pub user_message: String,
    pub allowed_tools: Vec<String>,
    pub previous_events: Vec<RuntimeEvent>,
}

/// One tool call selected by planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerToolCall {
    pub tool_name: String,
    pub args: BTreeMap<String, serde_json::Value>,
}

/// Planner model output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannerTurnOutput {
    pub thoughts: Option<String>,
    pub tool_calls: Vec<PlannerToolCall>,
}

/// Input payload for quarantine extraction model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuarantineExtractInput {
    pub run_id: RunId,
    pub prompt: String,
    pub enum_registry: BTreeMap<String, BTreeSet<String>>,
}

/// Output payload for quarantine extraction model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuarantineExtractOutput {
    pub value: TypedValue,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn assert_matches_schema(schema_json: &str, instance: &Value) {
        let schema: Value = serde_json::from_str(schema_json).expect("parse schema");
        let validator = jsonschema::validator_for(&schema).expect("compile schema");
        validator
            .validate(instance)
            .expect("instance should satisfy schema");
    }

    #[test]
    fn approval_requested_event_json_round_trip() {
        let event = ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("apr_1".into()),
            run_id: RunId("run_1".into()),
            command_segments: vec![
                CommandSegment {
                    argv: vec!["echo".into(), "hello".into()],
                    operator_before: None,
                },
                CommandSegment {
                    argv: vec!["wc".into(), "-c".into()],
                    operator_before: Some(CompositionOperator::Pipe),
                },
            ],
            inferred_capabilities: vec![Capability {
                resource: Resource::Proc,
                action: Action::Exec,
                scope: "/usr/bin/wc".into(),
            }],
            blocked_rule_id: "rule.command.rm_rf".into(),
            reason: "deny_with_approval".into(),
            created_at_ms: 1_717_171_717_000,
        };

        let encoded = serde_json::to_string(&event).expect("serialize");
        let decoded: ApprovalRequestedEvent = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, event);
    }

    #[test]
    fn approval_resolved_event_json_round_trip() {
        let event = ApprovalResolvedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("apr_2".into()),
            run_id: RunId("run_2".into()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1_717_171_718_000,
        };

        let encoded = serde_json::to_string(&event).expect("serialize");
        let decoded: ApprovalResolvedEvent = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, event);
    }

    #[test]
    fn approval_requested_event_matches_schema() {
        let event = ApprovalRequestedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("apr_schema_1".into()),
            run_id: RunId("run_schema_1".into()),
            command_segments: vec![CommandSegment {
                argv: vec!["rm".into(), "-rf".into(), "/tmp/demo".into()],
                operator_before: None,
            }],
            inferred_capabilities: vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/demo".into(),
            }],
            blocked_rule_id: "rule.command.rm_rf".into(),
            reason: "requires approval".into(),
            created_at_ms: 1_717_171_800_000,
        };
        let instance = serde_json::to_value(event).expect("serialize event");
        let schema = include_str!("../../../schemas/approval-requested-event.schema.json");
        assert_matches_schema(schema, &instance);
    }

    #[test]
    fn approval_resolved_event_matches_schema() {
        let event = ApprovalResolvedEvent {
            schema_version: 1,
            request_id: ApprovalRequestId("apr_schema_2".into()),
            run_id: RunId("run_schema_2".into()),
            action: ApprovalAction::ApproveOnce,
            created_at_ms: 1_717_171_801_000,
        };
        let instance = serde_json::to_value(event).expect("serialize event");
        let schema = include_str!("../../../schemas/approval-resolved-event.schema.json");
        assert_matches_schema(schema, &instance);
    }

    #[test]
    fn runtime_event_json_round_trip() {
        let event = RuntimeEvent::PolicyEvaluated(PolicyEvaluatedEvent {
            schema_version: 1,
            run_id: RunId("run_3".into()),
            decision: PolicyDecision {
                kind: PolicyDecisionKind::DenyWithApproval,
                reason: "mutating command from untrusted context".into(),
                blocked_rule_id: Some("policy.integrity.001".into()),
            },
            inferred_capabilities: vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/out.txt".into(),
            }],
            trace_path: Some("/home/user/.sieve/logs/traces/run_3".into()),
            created_at_ms: 1_717_171_719_000,
        });

        let encoded = serde_json::to_string(&event).expect("serialize");
        let decoded: RuntimeEvent = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, event);
    }

    #[test]
    fn endorse_payload_json_round_trip() {
        let request = EndorseRequest {
            value_ref: ValueRef("v123".into()),
            target_integrity: Integrity::Trusted,
            reason: Some("user approved control use".into()),
        };
        let response = EndorseResponse {
            value_ref: ValueRef("v123e".into()),
            integrity: Integrity::Trusted,
        };

        let request_encoded = serde_json::to_string(&request).expect("serialize");
        let response_encoded = serde_json::to_string(&response).expect("serialize");
        let request_decoded: EndorseRequest =
            serde_json::from_str(&request_encoded).expect("deserialize");
        let response_decoded: EndorseResponse =
            serde_json::from_str(&response_encoded).expect("deserialize");

        assert_eq!(request_decoded, request);
        assert_eq!(response_decoded, response);
    }

    #[test]
    fn declassify_payload_json_round_trip() {
        let request = DeclassifyRequest {
            value_ref: ValueRef("v456".into()),
            sink: SinkKey("https://api.example.com/v1/upload".into()),
            reason: Some("user approved outbound upload".into()),
        };
        let response = DeclassifyResponse {
            value_ref: ValueRef("v456d".into()),
            allowed_sinks_added: vec![SinkKey("https://api.example.com/v1/upload".into())],
        };

        let request_encoded = serde_json::to_string(&request).expect("serialize");
        let response_encoded = serde_json::to_string(&response).expect("serialize");
        let request_decoded: DeclassifyRequest =
            serde_json::from_str(&request_encoded).expect("deserialize");
        let response_decoded: DeclassifyResponse =
            serde_json::from_str(&response_encoded).expect("deserialize");

        assert_eq!(request_decoded, request);
        assert_eq!(response_decoded, response);
    }
}
