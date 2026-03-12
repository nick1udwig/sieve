use crate::{RunId, SinkKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Capability resource dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Fs,
    Net,
    Proc,
    Env,
    Ipc,
}

/// Capability action dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

/// Narrow egress channel within one sink destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinkChannel {
    Body,
    Header,
    Query,
    Path,
    Cookie,
}

/// One sink permission scoped to destination and channel.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SinkPermission {
    pub sink: SinkKey,
    pub channel: SinkChannel,
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
    pub allowed_sinks: BTreeSet<SinkPermission>,
    pub capacity_type: CapacityType,
}
