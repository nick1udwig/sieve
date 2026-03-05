use serde::{Deserialize, Serialize};

/// Unix epoch milliseconds.
pub type UnixMillis = u64;

/// Stable identifier for one command run attempt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

/// Stable identifier for a labeled runtime value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ValueRef(pub String);

/// Stable identifier for one approval prompt lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ApprovalRequestId(pub String);

/// Canonical sink key (`scheme://host[:port]/path`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SinkKey(pub String);
