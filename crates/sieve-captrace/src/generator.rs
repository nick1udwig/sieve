mod cases;
mod help;
mod normalize;
mod reports;
mod templates;
mod trace_runner;
mod types;
mod util;

use self::cases::{
    builtin_case_templates, collect_seed_cases, dedupe_cases, dedupe_command_paths,
    enforce_known_case_coverage_guard, has_only_default_help_case,
    prune_known_unsupported_auto_cases,
};
use self::help::discover_help_driven_cases;
use self::normalize::{
    build_literal_template_replacements, choose_summary_outcome,
    normalize_existing_summary_outcome_for_definition,
};
use self::reports::build_subcommand_reports;
use self::templates::{abstract_argv_template, infer_command_path};
use self::util::{now_ms, sanitize_id};
use crate::error::CapTraceError;
use crate::fixture::create_fixture_layout;
use crate::planner::{CaseGenerationRequest, CaseGenerator};
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer};
use sieve_shell::BasicShellAnalyzer;
use sieve_types::CommandKnowledge;
use std::sync::Arc;

pub use self::normalize::derive_summary_from_trace;
pub use self::reports::{render_rust_snippet, write_definition_json};
pub use self::trace_runner::{BwrapTraceRunner, TraceRequest, TraceRunner};
pub use self::types::{
    GenerateDefinitionRequest, GeneratedCommandDefinition, GeneratedSubcommandReport,
    GeneratedSummaryOutcome, GeneratedVariantDefinition,
};

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
                    Ok(llm_cases) => cases.extend(llm_cases.into_iter().map(|argv_template| {
                        types::PlannedCase {
                            command_path: infer_command_path(
                                &argv_template,
                                &request.command,
                                &known_command_paths,
                            ),
                            argv_template,
                            source: types::CaseSource::Llm,
                        }
                    })),
                    Err(err) => notes.push(format!("llm case generation skipped: {err}")),
                }
            } else {
                notes.push("llm disabled: planner not configured".to_string());
            }
        }

        dedupe_cases(&mut cases);
        if cases.is_empty() {
            cases.push(types::PlannedCase {
                command_path: Vec::new(),
                argv_template: vec![request.command.clone(), "--help".to_string()],
                source: types::CaseSource::Builtin,
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
