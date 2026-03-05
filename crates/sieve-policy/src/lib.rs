#![forbid(unsafe_code)]

mod config;
mod engine;
mod normalize;
mod sink;

pub use config::{
    CapabilityPolicy, DenyRule, DenyRuleDecision, PolicyConfig, PolicyOptions, ViolationMode,
};
pub use engine::{PolicyConfigError, PolicyEngine, TomlPolicyEngine};
pub use sink::{
    canonicalize_net_origin_scope, canonicalize_sink_key, canonicalize_sink_set,
    SinkCanonicalizationError,
};

#[cfg(test)]
mod tests;
