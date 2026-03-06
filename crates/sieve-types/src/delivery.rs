use crate::InteractionModality;
use serde::{Deserialize, Serialize};

/// Outbound or inbound communication surface for a user turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryChannel {
    Stdin,
    Telegram,
}

/// Delivery metadata that should survive through response composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryContext {
    pub channel: DeliveryChannel,
    pub destination: Option<String>,
    pub input_modality: InteractionModality,
    pub response_modality: InteractionModality,
}
