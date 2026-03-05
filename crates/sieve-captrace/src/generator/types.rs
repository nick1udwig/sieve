use serde::{Deserialize, Serialize};
use sieve_types::{Capability, CommandKnowledge, CommandSummary};

#[derive(Debug, Clone)]
pub struct GenerateDefinitionRequest {
    pub command: String,
    pub seed_shell_cases: Vec<String>,
    pub include_llm_cases: bool,
    pub max_llm_cases: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedSummaryOutcome {
    pub knowledge: CommandKnowledge,
    pub summary: Option<CommandSummary>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedVariantDefinition {
    pub case_id: String,
    pub command_path: Vec<String>,
    pub argv_template: Vec<String>,
    pub argv_effective: Vec<String>,
    pub trace_path: Option<String>,
    pub exit_code: Option<i32>,
    pub attempted_capabilities: Vec<Capability>,
    pub trace_derived_summary: Option<CommandSummary>,
    pub summary_outcome: GeneratedSummaryOutcome,
    pub matches_existing_summary: Option<bool>,
    pub trace_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedSubcommandReport {
    pub command_path: Vec<String>,
    pub variant_count: usize,
    pub successful_variants: usize,
    pub failed_variants: usize,
    pub trace_error_count: usize,
    pub required_capabilities_union: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedCommandDefinition {
    pub schema_version: u16,
    pub command: String,
    pub generated_at_ms: u64,
    pub variants: Vec<GeneratedVariantDefinition>,
    pub subcommand_reports: Vec<GeneratedSubcommandReport>,
    pub notes: Vec<String>,
    pub rust_snippet: String,
}

#[derive(Debug, Clone)]
pub(super) struct PlannedCase {
    pub(super) argv_template: Vec<String>,
    pub(super) command_path: Vec<String>,
    pub(super) source: CaseSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CaseSource {
    Seed,
    Builtin,
    HelpDiscovery,
    Llm,
}

impl CaseSource {
    pub(super) fn is_seed(self) -> bool {
        matches!(self, CaseSource::Seed)
    }
}
