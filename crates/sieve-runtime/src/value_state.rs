use sieve_types::{
    ApprovalRequestId, ControlContext, DeclassifyStateTransition, EndorseStateTransition,
    Integrity, RuntimePolicyContext, SinkKey, SinkPermissionContext, ValueLabel, ValueRef,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValueStateError {
    #[error("value state lock poisoned")]
    LockPoisoned,
    #[error("unknown value reference: {0}")]
    UnknownValueRef(String),
}

#[derive(Default)]
pub(crate) struct RuntimeValueState {
    labels_by_value: BTreeMap<ValueRef, ValueLabel>,
}

impl RuntimeValueState {
    pub(crate) fn upsert_label(&mut self, value_ref: ValueRef, label: ValueLabel) {
        self.labels_by_value.insert(value_ref, label);
    }

    pub(crate) fn value_label(&self, value_ref: &ValueRef) -> Option<ValueLabel> {
        self.labels_by_value.get(value_ref).cloned()
    }

    pub(crate) fn has_any_labels(&self) -> bool {
        !self.labels_by_value.is_empty()
    }

    pub(crate) fn runtime_policy_context_for_control(
        &self,
        control_value_refs: BTreeSet<ValueRef>,
        endorsed_by: Option<ApprovalRequestId>,
    ) -> RuntimePolicyContext {
        let control_integrity = if control_value_refs.is_empty()
            || control_value_refs.iter().all(|value_ref| {
                self.labels_by_value
                    .get(value_ref)
                    .map(|label| label.integrity == Integrity::Trusted)
                    .unwrap_or(false)
            }) {
            Integrity::Trusted
        } else {
            Integrity::Untrusted
        };

        let allowed_sinks_by_value = self
            .labels_by_value
            .iter()
            .map(|(value_ref, label)| (value_ref.clone(), label.allowed_sinks.clone()))
            .collect();

        RuntimePolicyContext {
            control: ControlContext {
                integrity: control_integrity,
                value_refs: control_value_refs,
                endorsed_by,
            },
            sink_permissions: SinkPermissionContext {
                allowed_sinks_by_value,
            },
        }
    }

    pub(crate) fn apply_endorse_transition(
        &mut self,
        value_ref: ValueRef,
        to_integrity: Integrity,
        approved_by: Option<ApprovalRequestId>,
    ) -> Result<EndorseStateTransition, ValueStateError> {
        let label = self
            .labels_by_value
            .get_mut(&value_ref)
            .ok_or_else(|| ValueStateError::UnknownValueRef(value_ref.0.clone()))?;
        let from_integrity = label.integrity;
        label.integrity = to_integrity;

        Ok(EndorseStateTransition {
            value_ref,
            from_integrity,
            to_integrity,
            approved_by,
        })
    }

    pub(crate) fn apply_declassify_transition(
        &mut self,
        value_ref: ValueRef,
        sink: SinkKey,
        approved_by: Option<ApprovalRequestId>,
    ) -> Result<DeclassifyStateTransition, ValueStateError> {
        let label = self
            .labels_by_value
            .get_mut(&value_ref)
            .ok_or_else(|| ValueStateError::UnknownValueRef(value_ref.0.clone()))?;
        let sink_was_already_allowed = !label.allowed_sinks.insert(sink.clone());

        Ok(DeclassifyStateTransition {
            value_ref,
            sink,
            sink_was_already_allowed,
            approved_by,
        })
    }
}
