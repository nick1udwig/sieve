use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub(super) struct CodexResponsesRequest<'a> {
    pub(super) model: &'a str,
    pub(super) store: bool,
    pub(super) stream: bool,
    pub(super) instructions: String,
    pub(super) input: Vec<CodexInputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<Vec<CodexToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<CodexTextConfig>,
}

#[derive(Serialize)]
pub(super) struct CodexInputMessage {
    pub(super) role: String,
    pub(super) content: Vec<CodexContentPart>,
}

#[derive(Serialize)]
pub(super) struct CodexContentPart {
    #[serde(rename = "type")]
    pub(super) content_type: &'static str,
    pub(super) text: String,
}

#[derive(Serialize)]
pub(super) struct CodexTextConfig {
    pub(super) format: CodexTextFormat,
}

#[derive(Serialize)]
pub(super) struct CodexTextFormat {
    #[serde(rename = "type")]
    pub(super) format_type: &'static str,
    pub(super) name: String,
    pub(super) strict: bool,
    pub(super) schema: Value,
}

#[derive(Serialize)]
pub(super) struct CodexToolDefinition {
    #[serde(rename = "type")]
    pub(super) tool_type: &'static str,
    pub(super) name: String,
    pub(super) parameters: Value,
    pub(super) strict: bool,
}
