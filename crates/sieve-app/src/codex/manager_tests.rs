use super::*;
use async_trait::async_trait;
use sieve_runtime::{InProcessApprovalBus, RuntimeEventLog};
use sieve_types::{
    ApprovalAction, ApprovalPromptKind, ApprovalRequestedEvent, ApprovalResolvedEvent,
    CodexExecRequest, CodexSandboxMode, CodexSessionRequest, CodexTurnStatus, PlannerCodexSession,
    RuntimeEvent,
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

include!("manager_tests/shapes.rs");
include!("manager_tests/requests.rs");
include!("manager_tests/sessions.rs");
include!("manager_tests/approvals.rs");
include!("manager_tests/support.rs");
