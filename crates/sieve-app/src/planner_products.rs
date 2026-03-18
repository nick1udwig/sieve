use serde::Serialize;
use serde_json::Value;
use sieve_runtime::{
    MainlineArtifactKind, MainlineRunReport, PlannerToolResult, RuntimeDisposition,
};
use std::collections::BTreeMap;
use std::fs;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PlannerIntermediateProductSummary {
    pub product_ref: String,
    pub product_kind: String,
    pub tool_family: String,
    pub resource_kind: String,
    pub item_count: usize,
    pub detail_fetch_hint: PlannerDetailFetchHint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action_class", rename_all = "snake_case")]
pub(crate) enum PlannerDetailFetchHint {
    DetailFetch {
        tool: &'static str,
        placeholder_format: String,
        recommended_command_prefix: &'static str,
        recommended_param_shape: GwsMessageMetadataParamShape,
        recommended_count: usize,
    },
    SchemaFollowup {
        schema_target: String,
        command_prefix: String,
        note: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GwsMessageMetadataParamShape {
    #[serde(rename = "userId")]
    pub user_id: &'static str,
    pub id: String,
    pub format: &'static str,
    #[serde(rename = "metadataHeaders")]
    pub metadata_headers: [&'static str; 3],
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PlannerOpaqueHandleStore {
    next_product_id: usize,
    placeholder_values: BTreeMap<String, String>,
}

impl PlannerOpaqueHandleStore {
    pub(crate) fn record_step_products(
        &mut self,
        step_results: &[PlannerToolResult],
    ) -> Vec<PlannerIntermediateProductSummary> {
        let mut products = Vec::new();
        for result in step_results {
            if let Some(product) = extract_gws_schema_cli_shape(result, self.next_product_id + 1) {
                self.next_product_id = self.next_product_id.saturating_add(1);
                products.push(product);
                continue;
            }
            let Some(handles) = extract_gws_gmail_message_handles(result) else {
                continue;
            };
            if handles.is_empty() {
                continue;
            }
            self.next_product_id = self.next_product_id.saturating_add(1);
            let product_ref = format!("gws-gmail-message-{}", self.next_product_id);
            let item_count = handles.len();
            for (index, handle) in handles.into_iter().enumerate() {
                self.placeholder_values
                    .insert(format!("[[handle:{product_ref}:{index}]]"), handle);
            }
            products.push(PlannerIntermediateProductSummary {
                product_ref: product_ref.clone(),
                product_kind: "handle_list".to_string(),
                tool_family: "gws".to_string(),
                resource_kind: "gmail_message".to_string(),
                item_count,
                detail_fetch_hint: PlannerDetailFetchHint::DetailFetch {
                    tool: "bash",
                    placeholder_format: format!("[[handle:{product_ref}:<index>]]"),
                    recommended_command_prefix: "gws gmail users messages get --params",
                    recommended_param_shape: GwsMessageMetadataParamShape {
                        user_id: "me",
                        id: format!("[[handle:{product_ref}:<index>]]"),
                        format: "metadata",
                        metadata_headers: ["From", "Subject", "Date"],
                    },
                    recommended_count: 5,
                },
            });
        }
        products
    }

    pub(crate) fn placeholder_values(&self) -> BTreeMap<String, String> {
        self.placeholder_values.clone()
    }
}

fn extract_gws_gmail_message_handles(result: &PlannerToolResult) -> Option<Vec<String>> {
    let PlannerToolResult::Bash {
        command,
        disposition: RuntimeDisposition::ExecuteMainline(report),
    } = result
    else {
        return None;
    };
    if !is_gws_gmail_messages_list(command) {
        return None;
    }
    extract_handles_from_report(report)
}

fn is_gws_gmail_messages_list(command: &str) -> bool {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    matches!(
        tokens.as_slice(),
        ["gws", "gmail", "users", "messages", "list", ..]
    )
}

fn extract_handles_from_report(report: &MainlineRunReport) -> Option<Vec<String>> {
    let stdout_artifact = report.artifacts.iter().find(|artifact| {
        artifact.kind == MainlineArtifactKind::Stdout && artifact.byte_count > 0
    })?;
    let raw = fs::read_to_string(&stdout_artifact.path).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    let messages = value.get("messages")?.as_array()?;
    let mut handles = Vec::new();
    for message in messages {
        if let Some(id) = message.get("id").and_then(Value::as_str).map(str::trim) {
            if !id.is_empty() {
                handles.push(id.to_string());
            }
        }
    }
    Some(handles)
}

fn extract_gws_schema_cli_shape(
    result: &PlannerToolResult,
    next_product_id: usize,
) -> Option<PlannerIntermediateProductSummary> {
    let PlannerToolResult::Bash {
        command,
        disposition: RuntimeDisposition::ExecuteMainline(report),
    } = result
    else {
        return None;
    };
    if report.exit_code != Some(0) {
        return None;
    }
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    let ["gws", "schema", target] = tokens.as_slice() else {
        return None;
    };
    if !target.contains('.') {
        return None;
    }
    let cli_tokens = target.split('.').collect::<Vec<_>>();
    let product_ref = format!("gws-cli-shape-{next_product_id}");
    Some(PlannerIntermediateProductSummary {
        product_ref,
        product_kind: "cli_shape".to_string(),
        tool_family: "gws".to_string(),
        resource_kind: target.replace('.', "_"),
        item_count: cli_tokens.len(),
        detail_fetch_hint: PlannerDetailFetchHint::SchemaFollowup {
            schema_target: target.to_string(),
            command_prefix: format!("gws {}", cli_tokens.join(" ")),
            note: "GWS schema targets are dotted; CLI API calls use space-separated segments.",
        },
    })
}
