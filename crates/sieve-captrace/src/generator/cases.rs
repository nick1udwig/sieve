use super::templates::infer_command_path;
use super::types::{CaseSource, PlannedCase};
use crate::error::CapTraceError;
use crate::fixture::{TOKEN_IN_FILE, TOKEN_OUT_FILE, TOKEN_TMP_DIR};
use crate::command_match::argv_matches_command;
use sieve_command_summaries::{
    CommandSummarizer, DefaultCommandSummarizer, SummaryOutcome as ExistingSummaryOutcome,
};
use sieve_shell::{BasicShellAnalyzer, ShellAnalyzer};
use sieve_types::CommandKnowledge;
use std::collections::BTreeSet;
use std::path::Path;

pub(super) fn collect_seed_cases(
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

pub(super) fn builtin_case_templates(
    command: &str,
    known_command_paths: &[Vec<String>],
) -> Vec<PlannedCase> {
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

pub(super) fn has_only_default_help_case(cases: &[PlannedCase], command: &str) -> bool {
    cases.len() == 1
        && cases[0].argv_template.first().map(String::as_str) == Some(command)
        && cases[0].argv_template.get(1).map(String::as_str) == Some("--help")
}

pub(super) fn dedupe_command_paths(command_paths: &mut Vec<Vec<String>>) {
    let mut seen = BTreeSet::new();
    command_paths.retain(|path| seen.insert(path.join("\u{1f}")));
}

pub(super) fn prune_known_unsupported_auto_cases(
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

pub(super) fn enforce_known_case_coverage_guard(
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

pub(super) fn dedupe_cases(cases: &mut Vec<PlannedCase>) {
    let mut unique = BTreeSet::new();
    cases.retain(|case| unique.insert(case.argv_template.join("\u{1f}")));
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
