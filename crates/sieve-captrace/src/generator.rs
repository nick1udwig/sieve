#![forbid(unsafe_code)]

use crate::error::{io_err, trace_err, CapTraceError};
use crate::fixture::{
    create_fixture_layout, FixtureLayout, TOKEN_ARG, TOKEN_DATA, TOKEN_HEADER, TOKEN_IN_FILE,
    TOKEN_IN_FILE_2, TOKEN_KV, TOKEN_OUT_FILE, TOKEN_TMP_DIR, TOKEN_URL,
};
use crate::planner::{argv_matches_command, CaseGenerationRequest, CaseGenerator};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sieve_command_summaries::{
    CommandSummarizer, DefaultCommandSummarizer, SummaryOutcome as ExistingSummaryOutcome,
};
use sieve_quarantine::{BwrapQuarantineRunner, QuarantineNetworkMode, QuarantineRunner};
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use sieve_types::{
    Capability, CommandKnowledge, CommandSegment, CommandSummary, QuarantineReport,
    QuarantineRunRequest, Resource, RunId,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct TraceRequest {
    pub run_id: String,
    pub cwd: String,
    pub argv: Vec<String>,
}

#[async_trait]
pub trait TraceRunner: Send + Sync {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError>;
}

#[derive(Clone)]
pub struct BwrapTraceRunner {
    inner: BwrapQuarantineRunner,
}

impl BwrapTraceRunner {
    pub fn new(logs_root: PathBuf) -> Self {
        Self {
            inner: BwrapQuarantineRunner::new(logs_root),
        }
    }

    pub fn with_sandbox(
        logs_root: PathBuf,
        network_mode: QuarantineNetworkMode,
        writable_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            inner: BwrapQuarantineRunner::with_sandbox(logs_root, network_mode, writable_paths),
        }
    }
}

#[async_trait]
impl TraceRunner for BwrapTraceRunner {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError> {
        let report = self
            .inner
            .run(QuarantineRunRequest {
                run_id: RunId(request.run_id),
                cwd: request.cwd,
                command_segments: vec![CommandSegment {
                    argv: request.argv,
                    operator_before: None,
                }],
            })
            .await
            .map_err(trace_err)?;
        Ok(report)
    }
}

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
struct PlannedCase {
    argv_template: Vec<String>,
    command_path: Vec<String>,
    source: CaseSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaseSource {
    Seed,
    Builtin,
    HelpDiscovery,
    Llm,
}

impl CaseSource {
    fn is_seed(self) -> bool {
        matches!(self, CaseSource::Seed)
    }
}

#[derive(Debug, Clone)]
struct HelpNode {
    command_path: Vec<String>,
    sample_args_from_parent_usage: Vec<String>,
    help_text: String,
}

#[derive(Debug, Clone)]
struct HelpSubcommandSpec {
    name: String,
    usage_tail: Vec<String>,
}

#[derive(Debug, Clone)]
struct HelpFlagSpec {
    flag: String,
    takes_value: bool,
    value_hint: Option<String>,
}

const TEMPLATE_TOKEN_FILES: [&str; 4] = [
    TOKEN_TMP_DIR,
    TOKEN_IN_FILE,
    TOKEN_IN_FILE_2,
    TOKEN_OUT_FILE,
];
const TEMPLATE_TOKEN_GENERICS: [&str; 5] =
    [TOKEN_URL, TOKEN_HEADER, TOKEN_DATA, TOKEN_KV, TOKEN_ARG];

pub struct CapTraceGenerator {
    trace_runner: Arc<dyn TraceRunner>,
    case_generator: Option<Arc<dyn CaseGenerator>>,
    shell: BasicShellAnalyzer,
    summaries: DefaultCommandSummarizer,
}

impl CapTraceGenerator {
    pub fn new(
        trace_runner: Arc<dyn TraceRunner>,
        case_generator: Option<Arc<dyn CaseGenerator>>,
    ) -> Self {
        Self {
            trace_runner,
            case_generator,
            shell: BasicShellAnalyzer,
            summaries: DefaultCommandSummarizer,
        }
    }

    pub async fn generate(
        &self,
        request: GenerateDefinitionRequest,
    ) -> Result<GeneratedCommandDefinition, CapTraceError> {
        let fixture = create_fixture_layout()?;
        let mut notes = Vec::new();
        let mut known_command_paths = vec![Vec::new()];
        let mut cases = collect_seed_cases(
            &self.shell,
            &request.command,
            &request.seed_shell_cases,
            &known_command_paths,
            &mut notes,
        );

        if cases.is_empty() {
            cases.extend(builtin_case_templates(
                &request.command,
                &known_command_paths,
            ));
        }
        if has_only_default_help_case(&cases, &request.command) {
            match discover_help_driven_cases(&request.command) {
                Ok((discovered_cases, discovered_paths)) if !discovered_cases.is_empty() => {
                    known_command_paths.extend(discovered_paths);
                    notes.push(format!(
                        "discovered {} subcommand exercise cases from `{} --help`",
                        discovered_cases.len(),
                        request.command
                    ));
                    cases.extend(discovered_cases);
                }
                Ok(_) => {}
                Err(err) => notes.push(format!("subcommand discovery skipped: {err}")),
            }
        }
        dedupe_command_paths(&mut known_command_paths);

        if request.include_llm_cases {
            if let Some(generator) = &self.case_generator {
                match generator
                    .generate_cases(CaseGenerationRequest {
                        command: request.command.clone(),
                        max_cases: request.max_llm_cases.max(1),
                    })
                    .await
                {
                    Ok(llm_cases) => {
                        cases.extend(llm_cases.into_iter().map(|argv_template| PlannedCase {
                            command_path: infer_command_path(
                                &argv_template,
                                &request.command,
                                &known_command_paths,
                            ),
                            argv_template,
                            source: CaseSource::Llm,
                        }))
                    }
                    Err(err) => notes.push(format!("llm case generation skipped: {err}")),
                }
            } else {
                notes.push("llm disabled: planner not configured".to_string());
            }
        }

        dedupe_cases(&mut cases);
        if cases.is_empty() {
            cases.push(PlannedCase {
                command_path: Vec::new(),
                argv_template: vec![request.command.clone(), "--help".to_string()],
                source: CaseSource::Builtin,
            });
            notes.push("no cases discovered; fallback to `--help`".to_string());
        }

        let skipped_unsupported = prune_known_unsupported_auto_cases(
            &mut cases,
            &self.summaries,
            &request.command,
            &mut notes,
        );
        if skipped_unsupported > 0 {
            notes.push(format!(
                "filtered {skipped_unsupported} auto-generated case(s) with baseline-unsupported flags"
            ));
        }
        dedupe_cases(&mut cases);
        enforce_known_case_coverage_guard(&request.command, &cases, &self.summaries)?;

        let mut variants = Vec::new();
        for (idx, planned_case) in cases.into_iter().enumerate() {
            let raw_argv_template = planned_case.argv_template;
            let command_path = if planned_case.command_path.is_empty() {
                infer_command_path(&raw_argv_template, &request.command, &known_command_paths)
            } else {
                planned_case.command_path
            };
            let argv_template = abstract_argv_template(&raw_argv_template, &command_path);
            let argv_effective = fixture.apply_to_argv_template(&raw_argv_template);
            let run_id = format!(
                "captrace-{}-{}-{}",
                sanitize_id(&request.command),
                now_ms(),
                idx
            );
            let traced = self
                .trace_runner
                .trace(TraceRequest {
                    run_id: run_id.clone(),
                    cwd: "/tmp".to_string(),
                    argv: argv_effective.clone(),
                })
                .await;

            match traced {
                Ok(report) => {
                    let trace_derived_summary =
                        derive_summary_from_trace(&report.attempted_capabilities, &fixture);
                    let literal_replacements =
                        build_literal_template_replacements(&raw_argv_template, &argv_template);
                    let existing = normalize_existing_summary_outcome_for_definition(
                        self.summaries.summarize(&raw_argv_template),
                        &fixture,
                        &literal_replacements,
                    );
                    let (summary_outcome, matches_existing_summary) =
                        choose_summary_outcome(existing, &trace_derived_summary);
                    variants.push(GeneratedVariantDefinition {
                        case_id: run_id,
                        command_path,
                        argv_template,
                        argv_effective,
                        trace_path: Some(report.trace_path),
                        exit_code: report.exit_code,
                        attempted_capabilities: report.attempted_capabilities,
                        trace_derived_summary: Some(trace_derived_summary),
                        summary_outcome,
                        matches_existing_summary,
                        trace_error: None,
                    });
                }
                Err(err) => {
                    variants.push(GeneratedVariantDefinition {
                        case_id: run_id,
                        command_path,
                        argv_template,
                        argv_effective,
                        trace_path: None,
                        exit_code: None,
                        attempted_capabilities: Vec::new(),
                        trace_derived_summary: None,
                        summary_outcome: GeneratedSummaryOutcome {
                            knowledge: CommandKnowledge::Unknown,
                            summary: None,
                            reason: Some("trace failed".to_string()),
                        },
                        matches_existing_summary: None,
                        trace_error: Some(err.to_string()),
                    });
                }
            }
        }

        let subcommand_reports = build_subcommand_reports(&variants);
        let rust_snippet = render_rust_snippet(&request.command, &variants);
        Ok(GeneratedCommandDefinition {
            schema_version: 1,
            command: request.command,
            generated_at_ms: now_ms(),
            variants,
            subcommand_reports,
            notes,
            rust_snippet,
        })
    }
}

pub fn derive_summary_from_trace(
    attempted: &[Capability],
    fixture: &FixtureLayout,
) -> CommandSummary {
    let mut deduped = BTreeSet::new();
    let mut required_capabilities = Vec::new();

    for capability in attempted {
        if !should_keep_capability(capability, fixture) {
            continue;
        }
        let mut normalized = capability.clone();
        normalized.scope = normalize_capability_scope_for_definition(&normalized, fixture);
        let key = format!(
            "{:?}\u{1f}{:?}\u{1f}{}",
            normalized.resource, normalized.action, normalized.scope
        );
        if deduped.insert(key) {
            required_capabilities.push(normalized);
        }
    }

    CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    }
}

fn normalize_existing_summary_outcome_for_definition(
    mut outcome: ExistingSummaryOutcome,
    fixture: &FixtureLayout,
    literal_replacements: &[(String, String)],
) -> ExistingSummaryOutcome {
    if let Some(summary) = outcome.summary.as_mut() {
        normalize_command_summary_for_definition(summary, fixture, literal_replacements);
    }
    outcome
}

fn normalize_command_summary_for_definition(
    summary: &mut CommandSummary,
    fixture: &FixtureLayout,
    literal_replacements: &[(String, String)],
) {
    for capability in &mut summary.required_capabilities {
        let normalized_scope = normalize_capability_scope_for_definition(capability, fixture);
        capability.scope =
            apply_literal_template_replacements(&normalized_scope, literal_replacements);
    }
    dedupe_capabilities(&mut summary.required_capabilities);

    for check in &mut summary.sink_checks {
        check.sink.0 = normalize_sink_scope_for_definition(&check.sink.0, literal_replacements);
    }
}

fn dedupe_capabilities(capabilities: &mut Vec<Capability>) {
    let mut seen = BTreeSet::new();
    capabilities.retain(|capability| seen.insert(capability_key(capability)));
}

fn normalize_sink_scope_for_definition(
    scope: &str,
    literal_replacements: &[(String, String)],
) -> String {
    let replaced = apply_literal_template_replacements(scope, literal_replacements);
    if is_template_token(&replaced) || contains_template_token(&replaced) {
        return replaced;
    }
    if looks_like_url(&replaced) {
        return TOKEN_URL.to_string();
    }
    replaced
}

fn build_literal_template_replacements(
    raw_argv_template: &[String],
    argv_template: &[String],
) -> Vec<(String, String)> {
    let mut replacements = Vec::new();
    let mut seen = BTreeSet::new();
    for (raw, template) in raw_argv_template.iter().zip(argv_template.iter()) {
        if raw == template || raw.is_empty() {
            continue;
        }
        if !contains_template_token(template) {
            continue;
        }
        if seen.insert(raw.clone()) {
            replacements.push((raw.clone(), template.clone()));
        }
    }
    replacements.sort_by(|lhs, rhs| {
        rhs.0
            .len()
            .cmp(&lhs.0.len())
            .then_with(|| lhs.0.cmp(&rhs.0))
    });
    replacements
}

fn apply_literal_template_replacements(
    value: &str,
    literal_replacements: &[(String, String)],
) -> String {
    let mut out = value.to_string();
    for (literal, template) in literal_replacements {
        if literal.is_empty() || literal == template {
            continue;
        }
        out = out.replace(literal, template);
    }
    out
}

fn normalize_capability_scope_for_definition(
    capability: &Capability,
    fixture: &FixtureLayout,
) -> String {
    match capability.resource {
        Resource::Fs => fixture.normalize_scope_for_definition(&capability.scope),
        Resource::Net => normalize_network_scope_for_definition(&capability.scope),
        Resource::Ipc if capability.action == sieve_types::Action::Connect => {
            "ipc=local".to_string()
        }
        _ => capability.scope.clone(),
    }
}

fn normalize_network_scope_for_definition(scope: &str) -> String {
    if scope.starts_with("network=") {
        return scope.to_string();
    }

    if let Some(bucket) = network_scope_bucket(scope) {
        return format!("network={bucket}");
    }

    if looks_like_url(scope) {
        return "network=remote".to_string();
    }

    scope.to_string()
}

fn network_scope_bucket(scope: &str) -> Option<&'static str> {
    if let Some(address) = scope_field(scope, "address=") {
        return Some(if network_address_is_local(address) {
            "local"
        } else {
            "remote"
        });
    }

    let host = url_host(scope)?;
    Some(if network_address_is_local(host) {
        "local"
    } else {
        "remote"
    })
}

fn url_host(scope: &str) -> Option<&str> {
    let rest = scope
        .strip_prefix("http://")
        .or_else(|| scope.strip_prefix("https://"))?;
    let authority_end = rest
        .find(|ch| matches!(ch, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if let Some(bracketed) = host_port.strip_prefix('[') {
        let end = bracketed.find(']')?;
        return Some(&bracketed[..end]);
    }
    if let Some((host, _port)) = host_port.split_once(':') {
        if host.is_empty() {
            return None;
        }
        return Some(host);
    }
    Some(host_port)
}

fn network_address_is_local(address: &str) -> bool {
    let lowered = address.to_ascii_lowercase();
    if matches!(address, "127.0.0.1" | "::1" | "localhost" | "127.0.0.53") {
        return true;
    }
    if lowered == "localhost" || lowered.ends_with(".localhost") {
        return true;
    }
    if address.starts_with("127.") || address.starts_with("10.") || address.starts_with("192.168.")
    {
        return true;
    }
    if let Some(rest) = address.strip_prefix("172.") {
        if let Some(second_octet) = rest.split('.').next() {
            if let Ok(value) = second_octet.parse::<u8>() {
                if (16..=31).contains(&value) {
                    return true;
                }
            }
        }
    }
    if address.starts_with("169.254.") {
        return true;
    }
    lowered == "::1"
        || lowered.starts_with("fc")
        || lowered.starts_with("fd")
        || lowered.starts_with("fe80")
}

fn scope_field<'a>(scope: &'a str, key: &str) -> Option<&'a str> {
    let start = scope.find(key)? + key.len();
    let tail = &scope[start..];
    let end = tail.find(',').unwrap_or(tail.len());
    Some(tail[..end].trim())
}

pub fn write_definition_json(
    path: &Path,
    definition: &GeneratedCommandDefinition,
) -> Result<(), CapTraceError> {
    let encoded = serde_json::to_string_pretty(definition)
        .map_err(|err| CapTraceError::Io(err.to_string()))?;
    fs::write(path, encoded).map_err(io_err)
}

pub fn render_rust_snippet(command: &str, variants: &[GeneratedVariantDefinition]) -> String {
    let fn_name = format!(
        "summarize_{}_trace_generated",
        sanitize_id(command).replace('-', "_")
    );
    let mut out = String::new();
    out.push_str("// Generated by sieve-captrace.\n");
    out.push_str(
        "// Paste into crates/sieve-command-summaries/src/lib.rs and adapt matcher logic.\n",
    );
    out.push_str(&format!(
        "fn {fn_name}(argv: &[String]) -> Option<SummaryOutcome> {{\n"
    ));
    out.push_str("    fn captrace_template_token_matches(token: &str, value: &str) -> bool {\n");
    out.push_str("        match token {\n");
    out.push_str(&format!(
        "            {} => value.starts_with(\"http://\") || value.starts_with(\"https://\"),\n",
        rust_string_literal(TOKEN_URL)
    ));
    out.push_str(&format!(
        "            {} => value.contains(':'),\n",
        rust_string_literal(TOKEN_HEADER)
    ));
    out.push_str(&format!(
        "            {} => value.contains('='),\n",
        rust_string_literal(TOKEN_KV)
    ));
    out.push_str(&format!(
        "            {} | {} | {} | {} | {} | {} => !value.is_empty(),\n",
        rust_string_literal(TOKEN_TMP_DIR),
        rust_string_literal(TOKEN_IN_FILE),
        rust_string_literal(TOKEN_IN_FILE_2),
        rust_string_literal(TOKEN_OUT_FILE),
        rust_string_literal(TOKEN_DATA),
        rust_string_literal(TOKEN_ARG),
    ));
    out.push_str("            _ => false,\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("    fn captrace_arg_matches_template(expected: &str, actual: &str) -> bool {\n");
    out.push_str("        if expected == actual {\n");
    out.push_str("            return true;\n");
    out.push_str("        }\n");
    out.push_str("        for token in [\n");
    for token in TEMPLATE_TOKEN_FILES
        .iter()
        .chain(TEMPLATE_TOKEN_GENERICS.iter())
    {
        out.push_str(&format!("            {},\n", rust_string_literal(token)));
    }
    out.push_str("        ] {\n");
    out.push_str("            if expected == token {\n");
    out.push_str("                return captrace_template_token_matches(token, actual);\n");
    out.push_str("            }\n");
    out.push_str("            if let Some(index) = expected.find(token) {\n");
    out.push_str("                let prefix = &expected[..index];\n");
    out.push_str("                let suffix = &expected[index + token.len()..];\n");
    out.push_str(
        "                if actual.len() < prefix.len() + suffix.len() || !actual.starts_with(prefix) || !actual.ends_with(suffix) {\n",
    );
    out.push_str("                    continue;\n");
    out.push_str("                }\n");
    out.push_str(
        "                let middle = &actual[prefix.len()..actual.len().saturating_sub(suffix.len())];\n",
    );
    out.push_str("                return captrace_template_token_matches(token, middle);\n");
    out.push_str("            }\n");
    out.push_str("        }\n");
    out.push_str("        false\n");
    out.push_str("    }\n");
    out.push_str(
        "    fn captrace_argv_matches_template(template: &[&str], argv: &[String]) -> bool {\n",
    );
    out.push_str("        if template.len() != argv.len() {\n");
    out.push_str("            return false;\n");
    out.push_str("        }\n");
    out.push_str("        template\n");
    out.push_str("            .iter()\n");
    out.push_str("            .zip(argv.iter())\n");
    out.push_str(
        "            .all(|(expected, actual)| captrace_arg_matches_template(expected, actual))\n",
    );
    out.push_str("    }\n");
    for variant in variants {
        let outcome = &variant.summary_outcome;
        if outcome.knowledge == CommandKnowledge::Known && outcome.summary.is_none() {
            continue;
        }
        out.push_str(&format!(
            "    // case_id: {}\n",
            rust_string_literal(&variant.case_id)
        ));
        if let Some(trace_path) = &variant.trace_path {
            out.push_str(&format!(
                "    // trace_path: {}\n",
                rust_string_literal(trace_path)
            ));
        }
        out.push_str("    let template = &[\n");
        for arg in &variant.argv_template {
            out.push_str(&format!("        {},\n", rust_string_literal(arg)));
        }
        out.push_str("    ];\n");
        out.push_str("    if captrace_argv_matches_template(template, argv) {\n");
        out.push_str("        return Some(SummaryOutcome {\n");
        out.push_str(&format!(
            "            knowledge: CommandKnowledge::{},\n",
            command_knowledge_variant(outcome.knowledge)
        ));
        if let Some(summary) = &outcome.summary {
            out.push_str("            summary: Some(CommandSummary {\n");
            out.push_str("                required_capabilities: vec![\n");
            for capability in &summary.required_capabilities {
                out.push_str("                    Capability {\n");
                out.push_str(&format!(
                    "                        resource: Resource::{},\n",
                    resource_variant(capability.resource)
                ));
                out.push_str(&format!(
                    "                        action: Action::{},\n",
                    action_variant(capability.action)
                ));
                out.push_str(&format!(
                    "                        scope: {}.to_string(),\n",
                    rust_string_literal(&capability.scope)
                ));
                out.push_str("                    },\n");
            }
            out.push_str("                ],\n");
            out.push_str("                sink_checks: vec![\n");
            for check in &summary.sink_checks {
                out.push_str("                    SinkCheck {\n");
                out.push_str(&format!(
                    "                        argument_name: {}.to_string(),\n",
                    rust_string_literal(&check.argument_name)
                ));
                out.push_str(&format!(
                    "                        sink: SinkKey({}.to_string()),\n",
                    rust_string_literal(&check.sink.0)
                ));
                out.push_str("                        value_refs: vec![\n");
                for value_ref in &check.value_refs {
                    out.push_str(&format!(
                        "                            ValueRef({}.to_string()),\n",
                        rust_string_literal(&value_ref.0)
                    ));
                }
                out.push_str("                        ],\n");
                out.push_str("                    },\n");
            }
            out.push_str("                ],\n");
            out.push_str("                unsupported_flags: vec![\n");
            for flag in &summary.unsupported_flags {
                out.push_str(&format!(
                    "                    {}.to_string(),\n",
                    rust_string_literal(flag)
                ));
            }
            out.push_str("                ],\n");
            out.push_str("            }),\n");
        } else {
            out.push_str("            summary: None,\n");
        }
        match outcome.reason.as_deref() {
            Some(reason) => out.push_str(&format!(
                "            reason: Some({}.to_string()),\n",
                rust_string_literal(reason)
            )),
            None => out.push_str("            reason: None,\n"),
        }
        out.push_str("        });\n");
        out.push_str("    }\n");
        out.push('\n');
    }
    out.push_str("    None\n");
    out.push_str("}\n");
    out
}

fn should_keep_capability(capability: &Capability, fixture: &FixtureLayout) -> bool {
    match capability.resource {
        Resource::Fs => {
            let root = fixture.root.to_string_lossy();
            capability.scope == root || capability.scope.starts_with(root.as_ref())
        }
        Resource::Net | Resource::Ipc => true,
        Resource::Proc | Resource::Env => false,
    }
}

fn choose_summary_outcome(
    existing: sieve_command_summaries::SummaryOutcome,
    trace_derived: &CommandSummary,
) -> (GeneratedSummaryOutcome, Option<bool>) {
    if existing.knowledge == CommandKnowledge::Known {
        let matches = existing.summary.as_ref() == Some(trace_derived);
        let reason = if matches {
            "matched existing command summary".to_string()
        } else {
            "used existing command summary (trace differed)".to_string()
        };
        return (
            GeneratedSummaryOutcome {
                knowledge: existing.knowledge,
                summary: existing.summary,
                reason: Some(reason),
            },
            Some(matches),
        );
    }

    if let Some(summary) = existing.summary {
        return (
            GeneratedSummaryOutcome {
                knowledge: existing.knowledge,
                summary: Some(summary),
                reason: existing.reason,
            },
            None,
        );
    }

    (
        GeneratedSummaryOutcome {
            knowledge: CommandKnowledge::Unknown,
            summary: Some(trace_derived.clone()),
            reason: Some("trace-derived candidate; no existing summary".to_string()),
        },
        None,
    )
}

fn capability_key(capability: &Capability) -> String {
    format!(
        "{:?}\u{1f}{:?}\u{1f}{}",
        capability.resource, capability.action, capability.scope
    )
}

fn collect_seed_cases(
    shell: &BasicShellAnalyzer,
    command: &str,
    seed_shell_cases: &[String],
    known_command_paths: &[Vec<String>],
    notes: &mut Vec<String>,
) -> Vec<PlannedCase> {
    let mut cases = Vec::new();
    for raw in seed_shell_cases {
        let parsed = shell.analyze_shell_lc_script(raw);
        let analysis = match parsed {
            Ok(analysis) => analysis,
            Err(err) => {
                notes.push(format!("seed case skipped (parse failed): {raw} ({err})"));
                continue;
            }
        };
        if analysis.knowledge != CommandKnowledge::Known || analysis.segments.len() != 1 {
            notes.push(format!(
                "seed case skipped (not single known command): {raw}"
            ));
            continue;
        }
        let argv = analysis.segments[0].argv.clone();
        if !argv_matches_command(&argv, command) {
            notes.push(format!(
                "seed case skipped (command mismatch): {raw} expected `{command}`"
            ));
            continue;
        }
        cases.push(PlannedCase {
            command_path: infer_command_path(&argv, command, known_command_paths),
            argv_template: argv,
            source: CaseSource::Seed,
        });
    }
    cases
}

fn builtin_case_templates(command: &str, known_command_paths: &[Vec<String>]) -> Vec<PlannedCase> {
    match command {
        "mkdir" => vec![planned_case(
            vec![
                "mkdir".to_string(),
                "-p".to_string(),
                format!("{TOKEN_TMP_DIR}/generated-dir"),
            ],
            command,
            known_command_paths,
        )],
        "touch" => vec![planned_case(
            vec!["touch".to_string(), TOKEN_OUT_FILE.to_string()],
            command,
            known_command_paths,
        )],
        "cp" => vec![planned_case(
            vec![
                "cp".to_string(),
                TOKEN_IN_FILE.to_string(),
                TOKEN_OUT_FILE.to_string(),
            ],
            command,
            known_command_paths,
        )],
        "mv" => vec![planned_case(
            vec![
                "mv".to_string(),
                TOKEN_IN_FILE.to_string(),
                format!("{TOKEN_TMP_DIR}/moved-file.txt"),
            ],
            command,
            known_command_paths,
        )],
        "rm" => vec![planned_case(
            vec![
                "rm".to_string(),
                "-f".to_string(),
                TOKEN_OUT_FILE.to_string(),
            ],
            command,
            known_command_paths,
        )],
        _ => vec![planned_case(
            vec![command.to_string(), "--help".to_string()],
            command,
            known_command_paths,
        )],
    }
}

fn planned_case(
    argv_template: Vec<String>,
    command: &str,
    known_command_paths: &[Vec<String>],
) -> PlannedCase {
    PlannedCase {
        command_path: infer_command_path(&argv_template, command, known_command_paths),
        argv_template,
        source: CaseSource::Builtin,
    }
}

fn has_only_default_help_case(cases: &[PlannedCase], command: &str) -> bool {
    cases.len() == 1
        && cases[0].argv_template.first().map(String::as_str) == Some(command)
        && cases[0].argv_template.get(1).map(String::as_str) == Some("--help")
}

fn discover_help_driven_cases(
    command: &str,
) -> Result<(Vec<PlannedCase>, Vec<Vec<String>>), String> {
    const HELP_DISCOVERY_MAX_DEPTH: usize = 4;
    const HELP_DISCOVERY_MAX_NODES: usize = 64;

    let mut queue = VecDeque::new();
    queue.push_back((Vec::<String>::new(), Vec::<String>::new()));

    let mut seen_paths = BTreeSet::new();
    let mut known_command_paths = Vec::new();
    let mut nodes = Vec::new();

    while let Some((command_path, sample_args_from_parent_usage)) = queue.pop_front() {
        if command_path.len() > HELP_DISCOVERY_MAX_DEPTH {
            continue;
        }
        let key = command_path.join("\u{1f}");
        if !seen_paths.insert(key) {
            continue;
        }
        if seen_paths.len() > HELP_DISCOVERY_MAX_NODES {
            break;
        }

        let help_text = match read_help_text(command, &command_path) {
            Ok(help_text) => help_text,
            Err(err) => {
                if command_path.is_empty() {
                    return Err(err);
                }
                continue;
            }
        };
        let subcommands = parse_subcommands_from_help(&help_text);
        known_command_paths.push(command_path.clone());
        nodes.push(HelpNode {
            command_path: command_path.clone(),
            sample_args_from_parent_usage,
            help_text,
        });

        if command_path.len() >= HELP_DISCOVERY_MAX_DEPTH {
            continue;
        }
        for subcommand in subcommands {
            if subcommand.name.eq_ignore_ascii_case("help") {
                continue;
            }
            let mut child_path = command_path.clone();
            child_path.push(subcommand.name);
            queue.push_back((
                child_path,
                sample_args_from_usage_tail(&subcommand.usage_tail),
            ));
            if seen_paths.len() + queue.len() > HELP_DISCOVERY_MAX_NODES {
                break;
            }
        }
    }

    let mut planned_cases = Vec::new();
    for node in nodes {
        let mut argv_prefix = vec![command.to_string()];
        argv_prefix.extend(node.command_path.clone());

        if !node.command_path.is_empty() {
            let mut help_case = argv_prefix.clone();
            help_case.push("--help".to_string());
            planned_cases.push(PlannedCase {
                command_path: node.command_path.clone(),
                argv_template: help_case,
                source: CaseSource::HelpDiscovery,
            });
        }

        if !node.sample_args_from_parent_usage.is_empty() {
            let mut exercise_case = argv_prefix.clone();
            exercise_case.extend(node.sample_args_from_parent_usage.clone());
            planned_cases.push(PlannedCase {
                command_path: node.command_path.clone(),
                argv_template: exercise_case,
                source: CaseSource::HelpDiscovery,
            });
        }

        planned_cases.extend(generate_flag_exercise_cases(&node, command));
    }

    Ok((planned_cases, known_command_paths))
}

fn read_help_text(command: &str, command_path: &[String]) -> Result<String, String> {
    let mut help_command = StdCommand::new(command);
    help_command.args(command_path);
    help_command.arg("--help");
    let output = help_command.output().map_err(|err| {
        format!(
            "`{} --help` failed: {err}",
            command_with_path(command, command_path)
        )
    })?;

    let mut help_text = String::new();
    help_text.push_str(String::from_utf8_lossy(&output.stdout).as_ref());
    if !output.stderr.is_empty() {
        if !help_text.is_empty() && !help_text.ends_with('\n') {
            help_text.push('\n');
        }
        help_text.push_str(String::from_utf8_lossy(&output.stderr).as_ref());
    }
    if help_text.trim().is_empty() {
        return Err(format!(
            "`{} --help` produced empty output",
            command_with_path(command, command_path)
        ));
    }
    Ok(help_text)
}

fn command_with_path(command: &str, command_path: &[String]) -> String {
    let mut joined = command.to_string();
    for segment in command_path {
        joined.push(' ');
        joined.push_str(segment);
    }
    joined
}

fn parse_subcommands_from_help(help_text: &str) -> Vec<HelpSubcommandSpec> {
    let mut in_commands_section = false;
    let mut found_any = false;
    let mut seen = BTreeSet::new();
    let mut subcommands = Vec::new();

    for line in help_text.lines() {
        let trimmed = line.trim();
        if !in_commands_section {
            if is_commands_header(trimmed) {
                in_commands_section = true;
            }
            continue;
        }

        if trimmed.is_empty() {
            if found_any {
                break;
            }
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            if found_any {
                break;
            }
            continue;
        }

        let command_spec = split_help_spec_column(trimmed);
        let mut parts = command_spec.split_whitespace();
        let Some(raw_token) = parts.next() else {
            continue;
        };
        let token = raw_token.trim_end_matches([',', ':']);
        if !is_subcommand_token(token) {
            if found_any {
                break;
            }
            continue;
        }

        let token = token.to_string();
        found_any = true;
        if seen.insert(token.clone()) {
            let usage_tail = parts.map(ToString::to_string).collect();
            subcommands.push(HelpSubcommandSpec {
                name: token,
                usage_tail,
            });
        }
    }

    subcommands
}

fn is_commands_header(line: &str) -> bool {
    matches!(
        line.to_ascii_lowercase().as_str(),
        "commands:" | "subcommands:" | "available commands:" | "available subcommands:"
    )
}

fn is_subcommand_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn split_help_spec_column(line: &str) -> &str {
    let bytes = line.as_bytes();
    for idx in 0..bytes.len() {
        if bytes[idx] == b'\t' {
            return line[..idx].trim_end();
        }
        if idx + 1 < bytes.len() && bytes[idx] == b' ' && bytes[idx + 1] == b' ' {
            return line[..idx].trim_end();
        }
    }
    line.trim_end()
}

fn sample_args_from_usage_tail(usage_tail: &[String]) -> Vec<String> {
    let mut args = Vec::new();
    for token in usage_tail {
        let normalized = normalize_usage_token(token);
        if normalized.is_empty() {
            continue;
        }

        if normalized.starts_with('<') && normalized.ends_with('>') {
            let placeholder = &normalized[1..normalized.len() - 1];
            args.push(sample_value_for_placeholder(placeholder));
            continue;
        }
        if normalized.starts_with("--") || normalized.starts_with('-') {
            args.push(normalized);
            continue;
        }

        if normalized.eq_ignore_ascii_case("query") {
            args.push(TOKEN_ARG.to_string());
            continue;
        }
    }
    args
}

fn normalize_usage_token(token: &str) -> String {
    token
        .trim_matches(|ch| matches!(ch, '[' | ']' | '(' | ')' | ',' | ':'))
        .to_string()
}

fn sample_value_for_placeholder(placeholder: &str) -> String {
    let lowered = placeholder.to_ascii_lowercase();
    if lowered.contains("query") {
        TOKEN_ARG.to_string()
    } else if lowered.contains("key") {
        TOKEN_ARG.to_string()
    } else if lowered.contains("value") {
        TOKEN_ARG.to_string()
    } else if lowered.contains("file") || lowered.contains("path") || lowered.contains("config") {
        TOKEN_IN_FILE.to_string()
    } else if lowered.contains("url") || lowered.contains("uri") || lowered.contains("endpoint") {
        TOKEN_URL.to_string()
    } else {
        TOKEN_ARG.to_string()
    }
}

fn generate_flag_exercise_cases(node: &HelpNode, command: &str) -> Vec<PlannedCase> {
    const HELP_FLAG_CASE_LIMIT_PER_COMMAND: usize = 2;

    let flags = parse_flags_from_help(&node.help_text);
    if flags.is_empty() {
        return Vec::new();
    }

    let mut chosen_cases = Vec::new();

    if let Some(query_flag) = flags.iter().find(|flag| is_query_flag(&flag.flag)) {
        let mut args = vec![query_flag.flag.clone()];
        if query_flag.takes_value {
            args.push(sample_value_for_flag(query_flag));
        }
        chosen_cases.push(args);
    }

    for flag in flags {
        if chosen_cases.len() >= HELP_FLAG_CASE_LIMIT_PER_COMMAND {
            break;
        }
        if is_help_flag(&flag.flag) || is_query_flag(&flag.flag) {
            continue;
        }

        let mut args = vec![flag.flag.clone()];
        if flag.takes_value {
            args.push(sample_value_for_flag(&flag));
        }
        chosen_cases.push(args);
    }

    let mut out = Vec::new();
    for args in chosen_cases {
        let mut argv_template = vec![command.to_string()];
        argv_template.extend(node.command_path.clone());
        argv_template.extend(args);
        out.push(PlannedCase {
            command_path: node.command_path.clone(),
            argv_template,
            source: CaseSource::HelpDiscovery,
        });
    }
    out
}

fn parse_flags_from_help(help_text: &str) -> Vec<HelpFlagSpec> {
    let mut seen = BTreeSet::new();
    let mut flags = Vec::new();

    for line in help_text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('-') {
            continue;
        }

        let spec = split_help_spec_column(trimmed);
        let aliases: Vec<&str> = spec.split(',').map(str::trim).collect();
        let mut parsed = Vec::new();

        for alias in aliases {
            let mut parts = alias.split_whitespace();
            let Some(flag_token_raw) = parts.next() else {
                continue;
            };
            let flag_token = flag_token_raw.trim_end_matches([',', ':']);
            if !flag_token.starts_with('-') || flag_token == "-" || flag_token == "--" {
                continue;
            }

            let value_hint = parts.next().map(|value| value.to_string());
            parsed.push((flag_token.to_string(), value_hint));
        }

        if parsed.is_empty() {
            continue;
        }

        let mut chosen = parsed[0].clone();
        for entry in &parsed {
            if entry.0.starts_with("--") {
                chosen = entry.clone();
            }
        }

        let takes_value = parsed.iter().any(|entry| entry.1.is_some());
        let value_hint = if chosen.1.is_some() {
            chosen.1.clone()
        } else {
            parsed.iter().find_map(|entry| entry.1.clone())
        };

        if seen.insert(chosen.0.clone()) {
            flags.push(HelpFlagSpec {
                flag: chosen.0,
                takes_value,
                value_hint,
            });
        }
    }

    flags
}

fn is_help_flag(flag: &str) -> bool {
    matches!(flag, "-h" | "--help")
}

fn is_query_flag(flag: &str) -> bool {
    matches!(flag, "-q" | "--q" | "--query")
}

fn sample_value_for_flag(flag: &HelpFlagSpec) -> String {
    let lowered = flag.flag.to_ascii_lowercase();
    if lowered.contains("query") || lowered == "--q" || lowered == "-q" {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("count") || lowered.contains("offset") || lowered.contains("retries") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("country") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("search-lang") || lowered.contains("lang") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("ui-lang") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("safesearch") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("freshness") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("timeout") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("cache-ttl") || lowered.contains("ttl") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("output") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("api-key-file") || lowered == "-config" {
        return TOKEN_IN_FILE.to_string();
    }
    if lowered.contains("config") {
        return TOKEN_IN_FILE.to_string();
    }
    if lowered.contains("cache-dir") || lowered.contains("temp-dir") {
        return TOKEN_TMP_DIR.to_string();
    }
    if lowered.contains("param") {
        return TOKEN_KV.to_string();
    }
    if lowered.contains("goggle") {
        return TOKEN_URL.to_string();
    }
    if lowered.contains("api-key") {
        return TOKEN_ARG.to_string();
    }
    if lowered.contains("version") {
        return TOKEN_ARG.to_string();
    }
    if let Some(hint) = flag.value_hint.as_deref() {
        let hint_lowered = hint.to_ascii_lowercase();
        if hint_lowered.contains("int") {
            return TOKEN_ARG.to_string();
        }
        if hint_lowered.contains("duration") {
            return TOKEN_ARG.to_string();
        }
        if hint_lowered.contains("value") {
            return TOKEN_ARG.to_string();
        }
    }
    TOKEN_ARG.to_string()
}

fn dedupe_command_paths(command_paths: &mut Vec<Vec<String>>) {
    let mut seen = BTreeSet::new();
    command_paths.retain(|path| seen.insert(path.join("\u{1f}")));
}

fn abstract_argv_template(argv_template: &[String], command_path: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv_template.len());
    let mut value_kind_from_prev_flag: Option<&str> = None;
    let prefix_len = 1 + command_path.len();

    for (index, arg) in argv_template.iter().enumerate() {
        if index == 0 || index < prefix_len {
            out.push(arg.clone());
            continue;
        }

        if is_template_token(arg) {
            out.push(arg.clone());
            value_kind_from_prev_flag = None;
            continue;
        }
        if contains_template_token(arg) {
            out.push(arg.clone());
            value_kind_from_prev_flag = None;
            continue;
        }

        if let Some(kind) = value_kind_from_prev_flag.take() {
            out.push(placeholder_for_value_kind(kind).to_string());
            continue;
        }

        if let Some(kind) = value_kind_for_separate_flag(arg) {
            out.push(arg.clone());
            value_kind_from_prev_flag = Some(kind);
            continue;
        }

        if arg.starts_with('-') {
            if let Some((flag, value)) = split_flag_value(arg) {
                if let Some(kind) = value_kind_for_inline_flag(flag) {
                    out.push(format!("{flag}={}", placeholder_for_value_kind(kind)));
                } else if is_kv_like(value) {
                    out.push(format!("{flag}={TOKEN_KV}"));
                } else {
                    out.push(format!("{flag}={TOKEN_ARG}"));
                }
            } else {
                out.push(arg.clone());
            }
            continue;
        }

        out.push(abstract_positional_value(arg).to_string());
    }

    out
}

fn value_kind_for_separate_flag(flag: &str) -> Option<&'static str> {
    match flag {
        "-H" | "--header" => Some("header"),
        "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--data-urlencode"
        | "--json" => Some("data"),
        "--param" => Some("kv"),
        "--url" | "--goggle" => Some("url"),
        "--request" | "-X" => Some("arg"),
        _ => {
            if is_file_like_flag(flag) {
                Some("file")
            } else if is_url_like_flag(flag) {
                Some("url")
            } else {
                None
            }
        }
    }
}

fn value_kind_for_inline_flag(flag: &str) -> Option<&'static str> {
    match flag {
        "--header" => Some("header"),
        "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--data-urlencode"
        | "--json" => Some("data"),
        "--param" => Some("kv"),
        "--url" | "--goggle" => Some("url"),
        _ => {
            if is_file_like_flag(flag) {
                Some("file")
            } else if is_url_like_flag(flag) {
                Some("url")
            } else {
                None
            }
        }
    }
}

fn is_file_like_flag(flag: &str) -> bool {
    let lowered = flag.to_ascii_lowercase();
    lowered.contains("file")
        || lowered.contains("path")
        || lowered.contains("config")
        || lowered == "-o"
        || lowered == "--output"
}

fn is_url_like_flag(flag: &str) -> bool {
    let lowered = flag.to_ascii_lowercase();
    lowered.contains("url") || lowered.contains("endpoint")
}

fn placeholder_for_value_kind(kind: &str) -> &'static str {
    match kind {
        "header" => TOKEN_HEADER,
        "data" => TOKEN_DATA,
        "kv" => TOKEN_KV,
        "url" => TOKEN_URL,
        "file" => TOKEN_IN_FILE,
        _ => TOKEN_ARG,
    }
}

fn abstract_positional_value(value: &str) -> &'static str {
    if looks_like_url(value) {
        return TOKEN_URL;
    }
    if is_kv_like(value) {
        return TOKEN_KV;
    }
    TOKEN_ARG
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn is_kv_like(value: &str) -> bool {
    value.contains('=') && !value.starts_with('-')
}

fn split_flag_value(flag: &str) -> Option<(&str, &str)> {
    let eq = flag.find('=')?;
    Some((&flag[..eq], &flag[eq + 1..]))
}

fn is_template_token(value: &str) -> bool {
    TEMPLATE_TOKEN_FILES
        .iter()
        .chain(TEMPLATE_TOKEN_GENERICS.iter())
        .any(|token| value == *token)
}

fn contains_template_token(value: &str) -> bool {
    TEMPLATE_TOKEN_FILES
        .iter()
        .chain(TEMPLATE_TOKEN_GENERICS.iter())
        .any(|token| value.contains(token))
}

fn infer_command_path(
    argv_template: &[String],
    command: &str,
    known_command_paths: &[Vec<String>],
) -> Vec<String> {
    let args_after_command = args_after_command(argv_template, command);
    if args_after_command.is_empty() {
        return Vec::new();
    }

    let mut best_match: Option<Vec<String>> = None;
    for known_path in known_command_paths {
        if known_path.is_empty() || known_path.len() > args_after_command.len() {
            continue;
        }
        if args_after_command
            .iter()
            .take(known_path.len())
            .eq(known_path.iter())
        {
            if best_match
                .as_ref()
                .is_none_or(|existing| known_path.len() > existing.len())
            {
                best_match = Some(known_path.clone());
            }
        }
    }
    if let Some(best_match) = best_match {
        return best_match;
    }

    let mut inferred = Vec::new();
    for token in args_after_command {
        if token.starts_with('-') || token.starts_with("{{") {
            break;
        }
        if !is_subcommand_token(token) {
            break;
        }
        inferred.push(token.clone());
        if inferred.len() >= 3 {
            break;
        }
    }
    inferred
}

fn args_after_command<'a>(argv: &'a [String], command: &str) -> &'a [String] {
    let Some(first) = argv.first() else {
        return &[];
    };

    if token_matches_command(first, command) {
        return argv.get(1..).unwrap_or(&[]);
    }
    if first == "sudo" {
        if let Some(second) = argv.get(1) {
            if token_matches_command(second, command) {
                return argv.get(2..).unwrap_or(&[]);
            }
        }
    }
    &[]
}

fn token_matches_command(token: &str, command: &str) -> bool {
    if token == command || token.ends_with(&format!("/{command}")) {
        return true;
    }
    let command_basename = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str());
    command_basename
        .is_some_and(|basename| token == basename || token.ends_with(&format!("/{basename}")))
}

fn build_subcommand_reports(
    variants: &[GeneratedVariantDefinition],
) -> Vec<GeneratedSubcommandReport> {
    #[derive(Default)]
    struct ReportBuilder {
        command_path: Vec<String>,
        variant_count: usize,
        successful_variants: usize,
        failed_variants: usize,
        trace_error_count: usize,
        capability_union: BTreeMap<String, Capability>,
    }

    let mut by_path: BTreeMap<String, ReportBuilder> = BTreeMap::new();
    for variant in variants {
        let key = variant.command_path.join("\u{1f}");
        let report = by_path.entry(key).or_default();
        report.command_path = variant.command_path.clone();
        report.variant_count += 1;
        if variant.trace_error.is_some() {
            report.trace_error_count += 1;
        }
        if variant.exit_code == Some(0) {
            report.successful_variants += 1;
        } else {
            report.failed_variants += 1;
        }

        let summary = variant
            .summary_outcome
            .summary
            .as_ref()
            .or(variant.trace_derived_summary.as_ref());
        if let Some(summary) = summary {
            for capability in &summary.required_capabilities {
                let cap_key = format!(
                    "{:?}\u{1f}{:?}\u{1f}{}",
                    capability.resource, capability.action, capability.scope
                );
                report
                    .capability_union
                    .entry(cap_key)
                    .or_insert_with(|| capability.clone());
            }
        }
    }

    by_path
        .into_values()
        .map(|builder| GeneratedSubcommandReport {
            command_path: builder.command_path,
            variant_count: builder.variant_count,
            successful_variants: builder.successful_variants,
            failed_variants: builder.failed_variants,
            trace_error_count: builder.trace_error_count,
            required_capabilities_union: builder.capability_union.into_values().collect(),
        })
        .collect()
}

fn prune_known_unsupported_auto_cases(
    cases: &mut Vec<PlannedCase>,
    summaries: &DefaultCommandSummarizer,
    command: &str,
    notes: &mut Vec<String>,
) -> usize {
    let original = cases.clone();
    let mut filtered = Vec::with_capacity(cases.len());
    let mut removed = 0usize;

    for case in cases.drain(..) {
        if case.source.is_seed() {
            filtered.push(case);
            continue;
        }

        let outcome = summaries.summarize(&case.argv_template);
        if outcome_has_unsupported_flags(&outcome) {
            removed += 1;
            continue;
        }
        filtered.push(case);
    }

    if filtered.is_empty() && removed > 0 {
        notes.push(format!(
            "unsupported-case filtering skipped for `{command}` because it would remove all discovered cases"
        ));
        *cases = original;
        return 0;
    }

    *cases = filtered;
    removed
}

fn enforce_known_case_coverage_guard(
    command: &str,
    cases: &[PlannedCase],
    summaries: &DefaultCommandSummarizer,
) -> Result<(), CapTraceError> {
    let auto_cases: Vec<&PlannedCase> =
        cases.iter().filter(|case| !case.source.is_seed()).collect();
    if auto_cases.is_empty() {
        return Ok(());
    }

    let known_auto_cases = auto_cases
        .iter()
        .filter(|case| {
            summaries.summarize(&case.argv_template).knowledge == CommandKnowledge::Known
        })
        .count();
    if known_auto_cases > 0 {
        return Ok(());
    }

    if !baseline_parser_recognizes_command(command, summaries) {
        return Ok(());
    }

    Err(CapTraceError::Llm(format!(
        "case coverage guard: generated no baseline-known cases for `{command}`; provide supported cases via --seed-case or improve planner output"
    )))
}

fn baseline_parser_recognizes_command(command: &str, summaries: &DefaultCommandSummarizer) -> bool {
    let command_token = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
        .to_string();
    let outcome = summaries.summarize(&[
        command_token,
        "--__sieve_captrace_invalid_flag__".to_string(),
    ]);
    outcome_has_unsupported_flags(&outcome)
}

fn outcome_has_unsupported_flags(outcome: &ExistingSummaryOutcome) -> bool {
    outcome
        .summary
        .as_ref()
        .is_some_and(|summary| !summary.unsupported_flags.is_empty())
}

fn dedupe_cases(cases: &mut Vec<PlannedCase>) {
    let mut unique = BTreeSet::new();
    cases.retain(|case| unique.insert(case.argv_template.join("\u{1f}")));
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn resource_variant(resource: Resource) -> &'static str {
    match resource {
        Resource::Fs => "Fs",
        Resource::Net => "Net",
        Resource::Proc => "Proc",
        Resource::Env => "Env",
        Resource::Ipc => "Ipc",
    }
}

fn command_knowledge_variant(knowledge: CommandKnowledge) -> &'static str {
    match knowledge {
        CommandKnowledge::Known => "Known",
        CommandKnowledge::Unknown => "Unknown",
        CommandKnowledge::Uncertain => "Uncertain",
    }
}

fn action_variant(action: sieve_types::Action) -> &'static str {
    match action {
        sieve_types::Action::Read => "Read",
        sieve_types::Action::Write => "Write",
        sieve_types::Action::Append => "Append",
        sieve_types::Action::Exec => "Exec",
        sieve_types::Action::Connect => "Connect",
    }
}

fn rust_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
