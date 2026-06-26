use super::support::{CurlNetTraceRunner, StubCaseGenerator, StubTraceRunner};
use crate::fixture::{
    FixtureLayout, TOKEN_IN_FILE, TOKEN_KV, TOKEN_OUT_FILE, TOKEN_TMP_DIR, TOKEN_URL,
};
use crate::{
    derive_summary_from_trace, render_rust_snippet, CapTraceGenerator, GenerateDefinitionRequest,
};
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer};
use sieve_types::{Action, Capability, CommandKnowledge, Resource};
use std::path::PathBuf;
use std::sync::Arc;

fn fixture_at(root: &str) -> FixtureLayout {
    let root = PathBuf::from(root);
    FixtureLayout {
        in_file: root.join("input.txt"),
        in_file_2: root.join("input-2.txt"),
        out_file: root.join("output.txt"),
        root,
    }
}

#[tokio::test]
async fn generates_mkdir_summary_in_sieve_shape() {
    let generator = CapTraceGenerator::new(
        Arc::new(StubTraceRunner),
        Some(Arc::new(StubCaseGenerator {
            cases: vec![vec![
                "mkdir".to_string(),
                "-p".to_string(),
                format!("{TOKEN_TMP_DIR}/generated-dir"),
            ]],
        })),
    );

    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: "mkdir".to_string(),
            seed_shell_cases: Vec::new(),
            include_llm_cases: true,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    let variant = definition
        .variants
        .iter()
        .find(|variant| {
            variant.argv_template
                == vec![
                    "mkdir".to_string(),
                    "-p".to_string(),
                    format!("{TOKEN_TMP_DIR}/generated-dir"),
                ]
        })
        .expect("mkdir variant");
    let summary = variant
        .summary_outcome
        .summary
        .clone()
        .expect("summary exists");

    let expected = DefaultCommandSummarizer.summarize(&[
        "mkdir".to_string(),
        "-p".to_string(),
        format!("{TOKEN_TMP_DIR}/generated-dir"),
    ]);
    assert_eq!(expected.knowledge, CommandKnowledge::Known);
    assert_eq!(summary, expected.summary.expect("expected summary"));
    assert!(definition
        .rust_snippet
        .contains("fn summarize_mkdir_trace_generated"));
    assert!(definition
        .rust_snippet
        .contains("scope: \"{{TMP_DIR}}/generated-dir\".to_string()"));
}

#[tokio::test]
async fn filters_non_fixture_paths_from_cp_trace() {
    let generator = CapTraceGenerator::new(
        Arc::new(StubTraceRunner),
        Some(Arc::new(StubCaseGenerator {
            cases: vec![vec![
                "cp".to_string(),
                TOKEN_IN_FILE.to_string(),
                TOKEN_OUT_FILE.to_string(),
            ]],
        })),
    );

    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: "cp".to_string(),
            seed_shell_cases: Vec::new(),
            include_llm_cases: true,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    let cp_variant = definition
        .variants
        .iter()
        .find(|variant| variant.argv_template.first().map(String::as_str) == Some("cp"))
        .expect("cp variant");
    let summary = cp_variant
        .summary_outcome
        .summary
        .clone()
        .expect("summary exists");
    let trace_summary = cp_variant
        .trace_derived_summary
        .clone()
        .expect("trace-derived summary exists");

    assert!(summary.required_capabilities.contains(&Capability {
        resource: Resource::Fs,
        action: Action::Read,
        scope: TOKEN_IN_FILE.to_string(),
    }));
    assert!(summary.required_capabilities.contains(&Capability {
        resource: Resource::Fs,
        action: Action::Write,
        scope: TOKEN_OUT_FILE.to_string(),
    }));
    assert_eq!(summary.required_capabilities.len(), 2);
    assert!(cp_variant.matches_existing_summary == Some(true));

    assert!(trace_summary.required_capabilities.contains(&Capability {
        resource: Resource::Fs,
        action: Action::Read,
        scope: TOKEN_IN_FILE.to_string(),
    }));
    assert!(trace_summary.required_capabilities.contains(&Capability {
        resource: Resource::Fs,
        action: Action::Write,
        scope: TOKEN_OUT_FILE.to_string(),
    }));
    assert!(!trace_summary
        .required_capabilities
        .iter()
        .any(|cap| cap.scope == "/etc/ld.so.cache"));
}

#[test]
fn fixture_scope_normalization_does_not_template_sibling_paths() {
    let fixture = fixture_at("/tmp/sieve-captrace-fixture");

    assert_eq!(
        fixture.normalize_scope_for_definition("/tmp/sieve-captrace-fixture"),
        TOKEN_TMP_DIR
    );
    assert_eq!(
        fixture.normalize_scope_for_definition("/tmp/sieve-captrace-fixture/subdir/file"),
        format!("{TOKEN_TMP_DIR}/subdir/file")
    );
    assert_eq!(
        fixture.normalize_scope_for_definition("/tmp/sieve-captrace-fixture-sibling/secret"),
        "/tmp/sieve-captrace-fixture-sibling/secret"
    );
}

#[test]
fn trace_summary_filters_fs_sibling_paths_outside_fixture_root() {
    let fixture = fixture_at("/tmp/sieve-captrace-fixture");
    let summary = derive_summary_from_trace(
        &[
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/sieve-captrace-fixture/input.txt".to_string(),
            },
            Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/tmp/sieve-captrace-fixture-sibling/secret".to_string(),
            },
        ],
        &fixture,
    );

    assert!(summary.required_capabilities.contains(&Capability {
        resource: Resource::Fs,
        action: Action::Read,
        scope: TOKEN_IN_FILE.to_string(),
    }));
    assert_eq!(summary.required_capabilities.len(), 1);
}

#[tokio::test]
async fn abstracts_variable_argv_values_and_network_scopes() {
    let generator = CapTraceGenerator::new(Arc::new(CurlNetTraceRunner), None);
    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: "curl".to_string(),
            seed_shell_cases: vec![
                "curl --param param1=value1 https://example.com/api?x=1".to_string()
            ],
            include_llm_cases: false,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    let variant = definition.variants.first().expect("variant");
    assert!(
        variant.argv_template.contains(&TOKEN_KV.to_string()),
        "expected kv placeholder in argv template"
    );
    assert!(
        variant.argv_template.contains(&TOKEN_URL.to_string()),
        "expected url placeholder in argv template"
    );
    assert!(
        !variant
            .argv_template
            .iter()
            .any(|arg| arg.contains("example.com")),
        "literal host should not be present in argv template"
    );

    let summary = variant
        .summary_outcome
        .summary
        .clone()
        .expect("summary should exist");
    assert!(
        summary.required_capabilities.is_empty(),
        "unsupported summary should remain baseline shape"
    );
    let trace_summary = variant
        .trace_derived_summary
        .as_ref()
        .expect("trace-derived summary should exist");
    assert!(
        trace_summary.required_capabilities.contains(&Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: "network=remote".to_string(),
        }),
        "expected generalized remote network capability from trace summary"
    );

    let snippet = render_rust_snippet("curl", &definition.variants);
    assert!(snippet.contains("captrace_argv_matches_template"));
    assert!(snippet.contains(TOKEN_URL));
}

#[tokio::test]
async fn normalizes_known_summary_net_write_and_sink_scope() {
    let generator = CapTraceGenerator::new(Arc::new(StubTraceRunner), None);
    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: "curl".to_string(),
            seed_shell_cases: vec![
                "curl --request put --url https://example.com/upload --data-binary blob"
                    .to_string(),
            ],
            include_llm_cases: false,
            max_llm_cases: 1,
        })
        .await
        .expect("definition should generate");

    let variant = definition.variants.first().expect("variant");
    let summary = variant
        .summary_outcome
        .summary
        .clone()
        .expect("summary should exist");

    assert!(
        summary.required_capabilities.contains(&Capability {
            resource: Resource::Net,
            action: Action::Write,
            scope: "network=remote".to_string(),
        }),
        "expected net write capability to be generalized"
    );
    assert!(
        summary
            .sink_checks
            .iter()
            .all(|check| check.sink.0 == TOKEN_URL),
        "sink checks should use URL placeholder instead of literal endpoint"
    );
    assert!(
        !summary
            .required_capabilities
            .iter()
            .any(|capability| capability.scope.contains("example.com")),
        "summary capabilities should not contain literal hostnames"
    );
}
