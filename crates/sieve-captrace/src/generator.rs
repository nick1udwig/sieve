#![forbid(unsafe_code)]

use crate::error::{io_err, trace_err, CapTraceError};
use crate::fixture::{
    create_fixture_layout, FixtureLayout, TOKEN_IN_FILE, TOKEN_OUT_FILE, TOKEN_TMP_DIR,
};
use crate::planner::{argv_matches_command, CaseGenerationRequest, CaseGenerator};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer};
use sieve_quarantine::{BwrapQuarantineRunner, QuarantineRunner};
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use sieve_types::{
    Capability, CommandKnowledge, CommandSegment, CommandSummary, QuarantineReport,
    QuarantineRunRequest, Resource, RunId,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
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
pub struct GeneratedCommandDefinition {
    pub schema_version: u16,
    pub command: String,
    pub generated_at_ms: u64,
    pub variants: Vec<GeneratedVariantDefinition>,
    pub notes: Vec<String>,
    pub rust_snippet: String,
}

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
        let mut cases = collect_seed_cases(
            &self.shell,
            &request.command,
            &request.seed_shell_cases,
            &mut notes,
        );

        if cases.is_empty() {
            cases.extend(builtin_case_templates(&request.command));
        }

        if request.include_llm_cases {
            if let Some(generator) = &self.case_generator {
                match generator
                    .generate_cases(CaseGenerationRequest {
                        command: request.command.clone(),
                        max_cases: request.max_llm_cases.max(1),
                    })
                    .await
                {
                    Ok(llm_cases) => cases.extend(llm_cases),
                    Err(err) => notes.push(format!("llm case generation skipped: {err}")),
                }
            } else {
                notes.push("llm disabled: planner not configured".to_string());
            }
        }

        dedupe_cases(&mut cases);
        if cases.is_empty() {
            cases.push(vec![request.command.clone(), "--help".to_string()]);
            notes.push("no cases discovered; fallback to `--help`".to_string());
        }

        let mut variants = Vec::new();
        for (idx, argv_template) in cases.into_iter().enumerate() {
            let argv_effective = fixture.apply_to_argv_template(&argv_template);
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
                    let existing = self.summaries.summarize(&argv_template);
                    let (summary_outcome, matches_existing_summary) =
                        choose_summary_outcome(existing, &trace_derived_summary);
                    variants.push(GeneratedVariantDefinition {
                        case_id: run_id,
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

        let rust_snippet = render_rust_snippet(&request.command, &variants);
        Ok(GeneratedCommandDefinition {
            schema_version: 1,
            command: request.command,
            generated_at_ms: now_ms(),
            variants,
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
        normalized.scope = fixture.normalize_scope_for_definition(&normalized.scope);
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
    for variant in variants {
        let Some(summary) = &variant.summary_outcome.summary else {
            continue;
        };
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
        out.push_str("    if argv == &vec![\n");
        for arg in &variant.argv_template {
            out.push_str(&format!(
                "        {}.to_string(),\n",
                rust_string_literal(arg)
            ));
        }
        out.push_str("    ] {\n");
        out.push_str("        return Some(known_outcome(CommandSummary {\n");
        out.push_str("            required_capabilities: vec![\n");
        for capability in &summary.required_capabilities {
            out.push_str("                Capability {\n");
            out.push_str(&format!(
                "                    resource: Resource::{},\n",
                resource_variant(capability.resource)
            ));
            out.push_str(&format!(
                "                    action: Action::{},\n",
                action_variant(capability.action)
            ));
            out.push_str(&format!(
                "                    scope: {}.to_string(),\n",
                rust_string_literal(&capability.scope)
            ));
            out.push_str("                },\n");
        }
        out.push_str("            ],\n");
        out.push_str("            sink_checks: vec![\n");
        for check in &summary.sink_checks {
            out.push_str("                SinkCheck {\n");
            out.push_str(&format!(
                "                    argument_name: {}.to_string(),\n",
                rust_string_literal(&check.argument_name)
            ));
            out.push_str(&format!(
                "                    sink: SinkKey({}.to_string()),\n",
                rust_string_literal(&check.sink.0)
            ));
            out.push_str("                    value_refs: vec![\n");
            for value_ref in &check.value_refs {
                out.push_str(&format!(
                    "                        ValueRef({}.to_string()),\n",
                    rust_string_literal(&value_ref.0)
                ));
            }
            out.push_str("                    ],\n");
            out.push_str("                },\n");
        }
        out.push_str("            ],\n");
        out.push_str("            unsupported_flags: vec![\n");
        for flag in &summary.unsupported_flags {
            out.push_str(&format!(
                "                {}.to_string(),\n",
                rust_string_literal(flag)
            ));
        }
        out.push_str("            ],\n");
        out.push_str("        }));\n");
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
        return (
            GeneratedSummaryOutcome {
                knowledge: existing.knowledge,
                summary: existing.summary,
                reason: Some("matched existing command summary".to_string()),
            },
            Some(matches),
        );
    }

    if existing.summary.is_some() {
        return (
            GeneratedSummaryOutcome {
                knowledge: existing.knowledge,
                summary: existing.summary,
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

fn collect_seed_cases(
    shell: &BasicShellAnalyzer,
    command: &str,
    seed_shell_cases: &[String],
    notes: &mut Vec<String>,
) -> Vec<Vec<String>> {
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
        cases.push(argv);
    }
    cases
}

fn builtin_case_templates(command: &str) -> Vec<Vec<String>> {
    match command {
        "mkdir" => vec![vec![
            "mkdir".to_string(),
            "-p".to_string(),
            format!("{TOKEN_TMP_DIR}/generated-dir"),
        ]],
        "touch" => vec![vec!["touch".to_string(), TOKEN_OUT_FILE.to_string()]],
        "cp" => vec![vec![
            "cp".to_string(),
            TOKEN_IN_FILE.to_string(),
            TOKEN_OUT_FILE.to_string(),
        ]],
        "mv" => vec![vec![
            "mv".to_string(),
            TOKEN_IN_FILE.to_string(),
            format!("{TOKEN_TMP_DIR}/moved-file.txt"),
        ]],
        "rm" => vec![vec![
            "rm".to_string(),
            "-f".to_string(),
            TOKEN_OUT_FILE.to_string(),
        ]],
        _ => vec![vec![command.to_string(), "--help".to_string()]],
    }
}

fn dedupe_cases(cases: &mut Vec<Vec<String>>) {
    let mut unique = BTreeSet::new();
    cases.retain(|case| unique.insert(case.join("\u{1f}")));
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
