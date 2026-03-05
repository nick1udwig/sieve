use crate::{Integrity, SinkKey, ValueRef};
use serde::{Deserialize, Serialize};

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
