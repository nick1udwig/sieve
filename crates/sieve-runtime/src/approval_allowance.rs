use sieve_policy::canonicalize_net_origin_scope;
use sieve_types::{Action, Capability, CommandKnowledge, CommandSegment, Resource};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ApprovalAllowanceKey {
    resource: Resource,
    action: Action,
    scope: String,
}

impl ApprovalAllowanceKey {
    pub(crate) fn for_capability(capability: &Capability) -> Self {
        Self {
            resource: capability.resource,
            action: capability.action,
            scope: canonical_approval_scope(capability),
        }
    }

    pub(crate) fn for_unknown_or_uncertain(
        kind: UnknownOrUncertain,
        command_segments: &[CommandSegment],
    ) -> Self {
        Self {
            resource: Resource::Proc,
            action: Action::Exec,
            scope: canonical_unknown_or_uncertain_scope(kind, command_segments),
        }
    }

    pub(crate) fn as_capability(&self) -> Capability {
        Capability {
            resource: self.resource,
            action: self.action,
            scope: self.scope.clone(),
        }
    }
}

fn canonical_approval_scope(capability: &Capability) -> String {
    match (capability.resource, capability.action) {
        (Resource::Net, Action::Connect) => canonical_net_origin_scope(&capability.scope)
            .unwrap_or_else(|| capability.scope.clone()),
        _ => capability.scope.clone(),
    }
}

fn canonical_net_origin_scope(scope: &str) -> Option<String> {
    canonicalize_net_origin_scope(scope)
}

fn canonical_unknown_or_uncertain_scope(
    kind: UnknownOrUncertain,
    command_segments: &[CommandSegment],
) -> String {
    let encoded = serde_json::to_string(command_segments).unwrap_or_else(|_| {
        command_segments
            .iter()
            .map(|segment| segment.argv.join(" "))
            .collect::<Vec<_>>()
            .join(" && ")
    });
    format!("{}::{encoded}", kind.to_blocked_rule_id())
}

#[derive(Clone, Copy)]
pub(crate) enum UnknownOrUncertain {
    Unknown,
    Uncertain,
}

impl UnknownOrUncertain {
    pub(crate) fn to_blocked_rule_id(self) -> &'static str {
        match self {
            Self::Unknown => "unknown_command_mode",
            Self::Uncertain => "uncertain_command_mode",
        }
    }

    pub(crate) fn to_knowledge(self) -> CommandKnowledge {
        match self {
            Self::Unknown => CommandKnowledge::Unknown,
            Self::Uncertain => CommandKnowledge::Uncertain,
        }
    }
}
