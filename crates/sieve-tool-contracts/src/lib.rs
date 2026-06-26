#![forbid(unsafe_code)]

mod descriptors;
mod schemas;
mod validate;

use serde::{Deserialize, Serialize};
use sieve_types::{
    AutomationRequest, CodexExecRequest, CodexSessionRequest, DeclassifyRequest, EndorseRequest,
    Integrity, ToolContractErrorCode, ToolContractValidationError, TOOL_CONTRACTS_VERSION_V1,
};
use std::fmt;
use std::ops::Deref;

pub use descriptors::{
    is_native_planner_tool, planner_exposed_tool_names, supported_tool_descriptors,
    tool_descriptor, ToolDescriptor, ToolExposure,
};
pub use schemas::{
    all_tool_args_schemas, emitted_schema_documents, planner_tool_call_schema,
    planner_turn_output_schema, tool_args_schema,
};
pub use validate::{validate, validate_at_index};

pub const TOOL_CONTRACTS_VERSION: u16 = TOOL_CONTRACTS_VERSION_V1;
pub const TOOL_AUTOMATION: &str = "automation";
pub const TOOL_BASH: &str = "bash";
pub const TOOL_CODEX_EXEC: &str = "codex_exec";
pub const TOOL_CODEX_SESSION: &str = "codex_session";
pub const TOOL_ENDORSE: &str = "endorse";
pub const TOOL_DECLASSIFY: &str = "declassify";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashArgs {
    pub cmd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexExecArgs {
    pub command: Vec<String>,
    pub sandbox: String,
    pub cwd: Option<String>,
    #[serde(default)]
    pub writable_roots: Vec<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexSessionArgs {
    pub session_id: Option<String>,
    pub instruction: String,
    pub sandbox: String,
    pub cwd: Option<String>,
    #[serde(default)]
    pub writable_roots: Vec<String>,
    #[serde(default)]
    pub local_images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationArgs {
    pub action: String,
    pub target: Option<String>,
    pub schedule: Option<serde_json::Value>,
    pub prompt: Option<String>,
    pub job_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndorseArgs {
    pub value_ref: String,
    pub target_integrity: ContractIntegrity,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclassifyArgs {
    pub value_ref: String,
    pub sink: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractIntegrity {
    Trusted,
    Untrusted,
}

impl From<ContractIntegrity> for Integrity {
    fn from(value: ContractIntegrity) -> Self {
        match value {
            ContractIntegrity::Trusted => Integrity::Trusted,
            ContractIntegrity::Untrusted => Integrity::Untrusted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypedCall {
    Automation(AutomationRequest),
    Bash(BashArgs),
    CodexExec(CodexExecRequest),
    CodexSession(CodexSessionRequest),
    Endorse(EndorseRequest),
    Declassify(DeclassifyRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    inner: Box<ContractErrorFields>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractErrorFields {
    pub code: ToolContractErrorCode,
    pub tool_call_index: usize,
    pub tool_name: String,
    pub argument_path: String,
    pub expected: Option<String>,
    pub found: Option<String>,
    pub message: String,
    pub hint: Option<String>,
}

impl Deref for ContractError {
    type Target = ContractErrorFields;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl fmt::Display for ContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ContractError {}

impl ContractError {
    pub fn with_tool_call_index(mut self, tool_call_index: usize) -> Self {
        self.inner.tool_call_index = tool_call_index;
        self
    }

    pub fn as_validation_error(&self) -> ToolContractValidationError {
        ToolContractValidationError {
            code: self.code,
            tool_call_index: self.tool_call_index,
            tool_name: self.tool_name.clone(),
            argument_path: self.argument_path.clone(),
            expected: self.expected.clone(),
            found: self.found.clone(),
            message: self.message.clone(),
            hint: self.hint.clone(),
            span: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractErrorParts {
    code: ToolContractErrorCode,
    argument_path: String,
    expected: Option<String>,
    found: Option<String>,
    message: String,
    hint: Option<String>,
}

impl ContractErrorParts {
    pub(crate) fn new(
        code: ToolContractErrorCode,
        argument_path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            argument_path: argument_path.into(),
            expected: None,
            found: None,
            message: message.into(),
            hint: None,
        }
    }

    pub(crate) fn expected(mut self, expected: impl Into<String>) -> Self {
        self.expected = Some(expected.into());
        self
    }

    pub(crate) fn found(mut self, found: impl Into<String>) -> Self {
        self.found = Some(found.into());
        self
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

pub fn supported_tools() -> Vec<&'static str> {
    supported_tool_descriptors()
        .iter()
        .map(|descriptor| descriptor.name)
        .collect()
}

pub(crate) fn make_error(
    tool_call_index: usize,
    tool_name: &str,
    parts: ContractErrorParts,
) -> ContractError {
    ContractError {
        inner: Box::new(ContractErrorFields {
            code: parts.code,
            tool_call_index,
            tool_name: tool_name.to_string(),
            argument_path: parts.argument_path,
            expected: parts.expected,
            found: parts.found,
            message: parts.message,
            hint: parts.hint,
        }),
    }
}

#[cfg(test)]
mod tests;
