use crate::{
    ApprovalRequestId, CapacityType, Integrity, SinkChannel, SinkKey, SinkPermission, ValueRef,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Frozen v1 version marker for strict tool-argument contracts.
pub const TOOL_CONTRACTS_VERSION_V1: u16 = 1;

/// Optional source location for contract diagnostics.
/// Coordinates are 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

/// Stable error code set for tool argument contract violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolContractErrorCode {
    UnknownTool,
    MissingRequiredField,
    UnknownField,
    InvalidType,
    InvalidValue,
    InvalidEnumVariant,
}

/// One compiler-like diagnostic for a planner tool-call validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContractValidationError {
    pub code: ToolContractErrorCode,
    pub tool_call_index: usize,
    pub tool_name: String,
    /// JSON pointer-like path within `tool_calls[i].args`.
    pub argument_path: String,
    pub expected: Option<String>,
    pub found: Option<String>,
    pub message: String,
    pub hint: Option<String>,
    pub span: Option<SourceSpan>,
}

/// Aggregate validation outcome for one planner turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolContractValidationReport {
    pub contract_version: u16,
    pub errors: Vec<ToolContractValidationError>,
}

/// Integrity context for command-control decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlContext {
    pub integrity: Integrity,
    pub value_refs: BTreeSet<ValueRef>,
    /// One-shot approval used to endorse this control context.
    pub endorsed_by: Option<ApprovalRequestId>,
}

/// Per-value sink permission context used by confidentiality checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SinkPermissionContext {
    pub allowed_sinks_by_value: BTreeMap<ValueRef, BTreeSet<SinkPermission>>,
    pub released_sinks_by_source_value: BTreeMap<ValueRef, BTreeSet<SinkPermission>>,
    pub capacity_type_by_value: BTreeMap<ValueRef, CapacityType>,
}

/// Runtime context consumed by policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePolicyContext {
    pub control: ControlContext,
    pub sink_permissions: SinkPermissionContext,
}

/// State transition produced by an approved `endorse` tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndorseStateTransition {
    pub value_ref: ValueRef,
    pub from_integrity: Integrity,
    pub to_integrity: Integrity,
    pub approved_by: Option<ApprovalRequestId>,
}

/// State transition produced by an approved `declassify` tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclassifyStateTransition {
    pub value_ref: ValueRef,
    pub release_value_ref: ValueRef,
    pub sink: SinkKey,
    pub channel: SinkChannel,
    pub release_value_existed: bool,
    pub approved_by: Option<ApprovalRequestId>,
}

/// Union of explicit tool state transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ExplicitToolStateTransition {
    Endorse(EndorseStateTransition),
    Declassify(DeclassifyStateTransition),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_contract_validation_report_json_round_trip() {
        let report = ToolContractValidationReport {
            contract_version: TOOL_CONTRACTS_VERSION_V1,
            errors: vec![ToolContractValidationError {
                code: ToolContractErrorCode::InvalidType,
                tool_call_index: 1,
                tool_name: "declassify".to_string(),
                argument_path: "/sink".to_string(),
                expected: Some("string".to_string()),
                found: Some("number".to_string()),
                message: "expected canonical sink key string".to_string(),
                hint: Some("pass https://host/path".to_string()),
                span: Some(SourceSpan {
                    line: 3,
                    column: 17,
                    end_line: 3,
                    end_column: 23,
                }),
            }],
        };

        let encoded = serde_json::to_string(&report).expect("serialize");
        let decoded: ToolContractValidationReport =
            serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, report);
    }

    #[test]
    fn runtime_policy_context_json_round_trip() {
        let mut control_refs = BTreeSet::new();
        control_refs.insert(ValueRef("v_control".to_string()));

        let mut allowed_sinks = BTreeSet::new();
        allowed_sinks.insert(SinkPermission {
            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: SinkChannel::Body,
        });

        let mut sink_permissions = BTreeMap::new();
        sink_permissions.insert(ValueRef("v_payload".to_string()), allowed_sinks);
        let mut released_sinks = BTreeMap::new();
        released_sinks.insert(
            ValueRef("v_source".to_string()),
            BTreeSet::from([SinkPermission {
                sink: SinkKey("https://api.example.com/v1/upload".to_string()),
                channel: SinkChannel::Body,
            }]),
        );
        let capacity_type_by_value = BTreeMap::from([
            (ValueRef("v_payload".to_string()), CapacityType::Enum),
            (ValueRef("v_source".to_string()), CapacityType::Enum),
        ]);

        let context = RuntimePolicyContext {
            control: ControlContext {
                integrity: Integrity::Trusted,
                value_refs: control_refs,
                endorsed_by: Some(ApprovalRequestId("apr_123".to_string())),
            },
            sink_permissions: SinkPermissionContext {
                allowed_sinks_by_value: sink_permissions,
                released_sinks_by_source_value: released_sinks,
                capacity_type_by_value,
            },
        };

        let encoded = serde_json::to_string(&context).expect("serialize");
        let decoded: RuntimePolicyContext = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, context);
    }

    #[test]
    fn explicit_tool_state_transition_json_round_trip() {
        let endorse = ExplicitToolStateTransition::Endorse(EndorseStateTransition {
            value_ref: ValueRef("v_control".to_string()),
            from_integrity: Integrity::Untrusted,
            to_integrity: Integrity::Trusted,
            approved_by: Some(ApprovalRequestId("apr_200".to_string())),
        });
        let declassify = ExplicitToolStateTransition::Declassify(DeclassifyStateTransition {
            value_ref: ValueRef("v_payload".to_string()),
            release_value_ref: ValueRef("v_release".to_string()),
            sink: SinkKey("https://api.example.com/v1/upload".to_string()),
            channel: SinkChannel::Body,
            release_value_existed: false,
            approved_by: Some(ApprovalRequestId("apr_201".to_string())),
        });

        let endorse_encoded = serde_json::to_string(&endorse).expect("serialize");
        let declassify_encoded = serde_json::to_string(&declassify).expect("serialize");
        let endorse_decoded: ExplicitToolStateTransition =
            serde_json::from_str(&endorse_encoded).expect("deserialize");
        let declassify_decoded: ExplicitToolStateTransition =
            serde_json::from_str(&declassify_encoded).expect("deserialize");

        assert_eq!(endorse_decoded, endorse);
        assert_eq!(declassify_decoded, declassify);
    }
}
