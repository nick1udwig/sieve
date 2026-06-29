use crate::ingress::TurnKind;
use crate::turn::TurnDeps;
use sieve_types::{InteractionModality, RunId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::turn) enum GeneratedAssistantMessage {
    Deliver {
        message: String,
        reply_to_session_id: Option<String>,
    },
    SuppressHeartbeat,
}

pub(in crate::turn) struct GenerateAssistantRequest<'a> {
    pub(in crate::turn) deps: &'a TurnDeps,
    pub(in crate::turn) run_id: &'a RunId,
    pub(in crate::turn) session_key: &'a str,
    pub(in crate::turn) trusted_user_message: &'a str,
    pub(in crate::turn) response_modality: InteractionModality,
    pub(in crate::turn) turn_kind: &'a TurnKind,
}
