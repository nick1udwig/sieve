use super::support::{
    make_temp_test_dir, write_failing_script, write_fake_help_script, StubCaseGenerator,
    StubTraceRunner,
};
use crate::fixture::{TOKEN_ARG, TOKEN_OUT_FILE, TOKEN_URL};
use crate::{CapTraceGenerator, GenerateDefinitionRequest};
use std::fs;
use std::sync::Arc;

#[tokio::test]
async fn accepts_basename_seed_case_when_command_is_absolute_path() {
    let generator = CapTraceGenerator::new(Arc::new(StubTraceRunner), None);

    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: "/root/git/brave-search/bravesearch".to_string(),
            seed_shell_cases: vec!["bravesearch search --help".to_string()],
            include_llm_cases: false,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    assert!(
        definition.variants.iter().any(|variant| {
            variant.argv_template
                == vec![
                    "bravesearch".to_string(),
                    "search".to_string(),
                    "--help".to_string(),
                ]
        }),
        "expected seed-case argv template to survive command matching"
    );
    assert!(
        !definition
            .notes
            .iter()
            .any(|note| note.contains("seed case skipped (command mismatch)")),
        "seed case should not be rejected as a command mismatch"
    );
}

#[tokio::test]
async fn discovers_subcommand_help_cases_from_help_output() {
    let temp_dir = make_temp_test_dir();
    let script_path = temp_dir.join("fake-bravesearch");
    let command = script_path.to_string_lossy().to_string();
    write_fake_help_script(&script_path);

    let generator = CapTraceGenerator::new(Arc::new(StubTraceRunner), None);
    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: command.clone(),
            seed_shell_cases: Vec::new(),
            include_llm_cases: false,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    assert!(
        definition.variants.iter().any(|variant| {
            variant.argv_template
                == vec![command.clone(), "search".to_string(), "--help".to_string()]
        }),
        "expected subcommand help case for search"
    );
    assert!(
        definition.variants.iter().any(|variant| {
            variant.argv_template == vec![command.clone(), "news".to_string(), "--help".to_string()]
        }),
        "expected subcommand help case for news"
    );
    assert!(
        definition.variants.iter().any(|variant| {
            variant.argv_template
                == vec![
                    command.clone(),
                    "search".to_string(),
                    "--query".to_string(),
                    TOKEN_ARG.to_string(),
                ]
        }),
        "expected flag exercise case for search"
    );
    assert!(
        definition
            .subcommand_reports
            .iter()
            .any(|report| report.command_path == vec!["search".to_string()]),
        "expected report entry for search subcommand"
    );
    assert!(
        definition
            .subcommand_reports
            .iter()
            .any(|report| report.command_path == vec!["news".to_string()]),
        "expected report entry for news subcommand"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn fails_when_auto_cases_have_no_baseline_known_matches() {
    let temp_dir = make_temp_test_dir();
    let script_path = temp_dir.join("curl");
    write_failing_script(&script_path);

    let generator = CapTraceGenerator::new(
        Arc::new(StubTraceRunner),
        Some(Arc::new(StubCaseGenerator {
            cases: vec![vec![
                "curl".to_string(),
                "-o".to_string(),
                TOKEN_OUT_FILE.to_string(),
                TOKEN_URL.to_string(),
            ]],
        })),
    );
    let command = script_path.to_string_lossy().to_string();
    let result = generator
        .generate(GenerateDefinitionRequest {
            command,
            seed_shell_cases: Vec::new(),
            include_llm_cases: true,
            max_llm_cases: 1,
        })
        .await;

    let err = result.expect_err("expected coverage guard failure");
    assert!(
        err.to_string().contains("case coverage guard"),
        "unexpected error: {err}"
    );
    let _ = fs::remove_dir_all(&temp_dir);
}
