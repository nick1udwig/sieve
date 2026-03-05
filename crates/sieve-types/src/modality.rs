use serde::{Deserialize, Serialize};

/// Turn-level modality for ingress and delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionModality {
    Text,
    Audio,
    Image,
}

/// Reason why response modality differs from input modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModalityOverrideReason {
    NotSupported,
    ToolFailure,
    Policy,
}

/// Explicit modality contract for one turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModalityContract {
    pub input: InteractionModality,
    pub response: InteractionModality,
    pub override_reason: Option<ModalityOverrideReason>,
}
