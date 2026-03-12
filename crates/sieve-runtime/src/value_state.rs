use sieve_types::{
    ApprovalRequestId, ControlContext, DeclassifyStateTransition, EndorseStateTransition,
    CapacityType, Integrity, RuntimePolicyContext, SinkChannel, SinkKey, SinkPermission,
    SinkPermissionContext, Source, ValueLabel, ValueRef,
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
    release_refs_by_grant: BTreeMap<DeclassifyGrantKey, ValueRef>,
    next_release_value_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DeclassifyGrantKey {
    source_value_ref: ValueRef,
    sink: SinkKey,
    channel: SinkChannel,
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
        let control_integrity = if control_value_refs.is_empty() {
            Integrity::Trusted
        } else if control_value_refs.iter().all(|value_ref| {
            self.labels_by_value
                .get(value_ref)
                .is_some_and(|label| {
                    label.integrity == Integrity::Trusted
                        && label.capacity_type != CapacityType::TrustedString
                })
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
        let mut released_sinks_by_source_value: BTreeMap<ValueRef, BTreeSet<SinkPermission>> =
            BTreeMap::new();
        for (grant, release_value_ref) in &self.release_refs_by_grant {
            if self.labels_by_value.contains_key(release_value_ref) {
                released_sinks_by_source_value
                    .entry(grant.source_value_ref.clone())
                    .or_default()
                    .insert(SinkPermission {
                        sink: grant.sink.clone(),
                        channel: grant.channel,
                    });
            }
        }
        let capacity_type_by_value = self
            .labels_by_value
            .iter()
            .map(|(value_ref, label)| (value_ref.clone(), label.capacity_type))
            .collect();

        RuntimePolicyContext {
            control: ControlContext {
                integrity: control_integrity,
                value_refs: control_value_refs,
                endorsed_by,
            },
            sink_permissions: SinkPermissionContext {
                allowed_sinks_by_value,
                released_sinks_by_source_value,
                capacity_type_by_value,
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
        channel: SinkChannel,
        approved_by: Option<ApprovalRequestId>,
    ) -> Result<DeclassifyStateTransition, ValueStateError> {
        let source_label = self
            .labels_by_value
            .get(&value_ref)
            .cloned()
            .ok_or_else(|| ValueStateError::UnknownValueRef(value_ref.0.clone()))?;
        let grant_key = DeclassifyGrantKey {
            source_value_ref: value_ref.clone(),
            sink: sink.clone(),
            channel,
        };
        if let Some(existing_release_ref) = self.release_refs_by_grant.get(&grant_key) {
            if self.labels_by_value.contains_key(existing_release_ref) {
                return Ok(DeclassifyStateTransition {
                    value_ref,
                    release_value_ref: existing_release_ref.clone(),
                    sink,
                    channel,
                    release_value_existed: true,
                    approved_by,
                });
            }
        }

        let release_value_ref = self.allocate_release_value_ref();
        let ValueLabel {
            integrity,
            provenance: source_provenance,
            capacity_type,
            ..
        } = source_label;
        let mut provenance = source_provenance;
        provenance.insert(Source::Tool {
            tool_name: "declassify".to_string(),
            inner_sources: BTreeSet::from([value_ref.0.clone()]),
        });
        let mut allowed_sinks = BTreeSet::new();
        allowed_sinks.insert(SinkPermission {
            sink: sink.clone(),
            channel,
        });
        self.labels_by_value.insert(
            release_value_ref.clone(),
            ValueLabel {
                integrity,
                provenance,
                allowed_sinks,
                capacity_type,
            },
        );
        self.release_refs_by_grant
            .insert(grant_key, release_value_ref.clone());

        Ok(DeclassifyStateTransition {
            value_ref,
            release_value_ref,
            sink,
            channel,
            release_value_existed: false,
            approved_by,
        })
    }

    fn allocate_release_value_ref(&mut self) -> ValueRef {
        self.next_release_value_id = self.next_release_value_id.saturating_add(1);
        ValueRef(format!("vrel_{}", self.next_release_value_id))
    }
}
