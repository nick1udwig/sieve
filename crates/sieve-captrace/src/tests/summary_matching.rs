use super::support::{shell_join, CurlNetTraceRunner, StubTraceRunner};
use crate::{render_rust_snippet, CapTraceGenerator, GenerateDefinitionRequest};
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer};
use sieve_types::{Action, Capability, CommandKnowledge, Resource};
use std::sync::Arc;

#[tokio::test]
async fn matches_existing_command_summaries_for_all_supported_cases() {
    let known_cases: Vec<Vec<&str>> = vec![
        vec!["trash", "{{TMP_DIR}}/demo"],
        vec!["cp", "{{IN_FILE}}", "{{OUT_FILE}}"],
        vec!["mv", "{{IN_FILE}}", "{{OUT_FILE}}"],
        vec!["mkdir", "-p", "{{TMP_DIR}}/work"],
        vec!["touch", "-d", "2026-01-01", "{{OUT_FILE}}"],
        vec!["chmod", "-R", "755", "{{OUT_FILE}}", "{{TMP_DIR}}/bin"],
        vec!["chown", "root:root", "{{OUT_FILE}}"],
        vec!["tee", "-a", "{{OUT_FILE}}"],
        vec!["ls", "-la"],
    ];

    let unknown_with_summary_cases: Vec<Vec<&str>> = vec![
        vec!["trash", "--bogus", "{{TMP_DIR}}/demo"],
        vec!["chown", "--from=user:group", "root:root", "{{OUT_FILE}}"],
        vec![
            "curl",
            "-X",
            "POST",
            "--upload-file",
            "payload.bin",
            "https://api.example.com/v1/upload",
        ],
        vec![
            "curl",
            "-X",
            "PUT",
            "-T",
            "payload.bin",
            "https://api.example.com/v1/upload",
        ],
        vec![
            "curl",
            "-X",
            "POST",
            "-F",
            "file=@payload.bin",
            "https://api.example.com/v1/upload",
        ],
    ];

    for case in known_cases {
        assert_case_matches_existing_summary(case).await;
    }
    for case in unknown_with_summary_cases {
        assert_case_matches_existing_summary(case).await;
    }
}

#[tokio::test]
async fn preserves_existing_unknown_summary_for_unsupported_flags() {
    let generator = CapTraceGenerator::new(Arc::new(CurlNetTraceRunner), None);
    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: "curl".to_string(),
            seed_shell_cases: vec!["curl -I https://example.com".to_string()],
            include_llm_cases: false,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    let variant = definition.variants.first().expect("curl -I variant");

    assert_eq!(variant.summary_outcome.knowledge, CommandKnowledge::Unknown);
    let summary = variant
        .summary_outcome
        .summary
        .clone()
        .expect("summary should exist for unsupported flags");
    assert!(
        summary.unsupported_flags.contains(&"-I".to_string()),
        "unsupported flags should be preserved"
    );
    assert!(
        summary.required_capabilities.is_empty(),
        "unknown summary from baseline should not be merged with trace capabilities"
    );
    let trace_summary = variant
        .trace_derived_summary
        .as_ref()
        .expect("trace summary should still be captured");
    assert!(
        trace_summary.required_capabilities.contains(&Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "network=remote".to_string(),
        }),
        "trace-derived net connect should remain available in trace summary"
    );
    let snippet = render_rust_snippet("curl", &definition.variants);
    assert!(
        snippet.contains("knowledge: CommandKnowledge::Unknown"),
        "generated snippet should preserve unknown outcome"
    );
}

async fn assert_case_matches_existing_summary(case: Vec<&str>) {
    let argv: Vec<String> = case.into_iter().map(ToString::to_string).collect();
    let command = argv.first().expect("command").to_string();
    let seed_case = shell_join(&argv);
    let generator = CapTraceGenerator::new(Arc::new(StubTraceRunner), None);

    let definition = generator
        .generate(GenerateDefinitionRequest {
            command,
            seed_shell_cases: vec![seed_case],
            include_llm_cases: false,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    let expected = DefaultCommandSummarizer.summarize(&argv);
    let variant = definition.variants.first().expect("variant for seed case");

    assert_eq!(
        variant.summary_outcome.knowledge, expected.knowledge,
        "knowledge mismatch for argv {:?}",
        variant.argv_template
    );
    assert_eq!(
        variant.summary_outcome.summary, expected.summary,
        "summary mismatch for argv {:?}",
        variant.argv_template
    );
}
