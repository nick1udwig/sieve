#![forbid(unsafe_code)]

use crate::fixture::{TOKEN_IN_FILE, TOKEN_OUT_FILE, TOKEN_TMP_DIR};
use crate::generator::{CapTraceGenerator, GenerateDefinitionRequest, TraceRequest, TraceRunner};
use crate::planner::{CaseGenerationRequest, CaseGenerator};
use crate::CapTraceError;
use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer};
use sieve_types::{Action, Capability, CommandKnowledge, QuarantineReport, Resource, RunId};
use std::sync::Arc;

struct StubTraceRunner;

#[async_trait]
impl TraceRunner for StubTraceRunner {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError> {
        let mut attempted_capabilities = Vec::new();
        if request.argv.first().map(String::as_str) == Some("mkdir") {
            if let Some(path) = request.argv.last() {
                attempted_capabilities.push(Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: path.clone(),
                });
            }
            attempted_capabilities.push(Capability {
                resource: Resource::Proc,
                action: Action::Exec,
                scope: "/usr/bin/mkdir".to_string(),
            });
        } else if request.argv.first().map(String::as_str) == Some("cp") {
            attempted_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: request.argv.get(1).cloned().unwrap_or_default(),
            });
            attempted_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: request.argv.get(2).cloned().unwrap_or_default(),
            });
            attempted_capabilities.push(Capability {
                resource: Resource::Fs,
                action: Action::Read,
                scope: "/etc/ld.so.cache".to_string(),
            });
        }

        Ok(QuarantineReport {
            run_id: RunId(request.run_id),
            trace_path: "/tmp/stub-trace".to_string(),
            stdout_path: None,
            stderr_path: None,
            attempted_capabilities,
            exit_code: Some(0),
        })
    }
}

struct StubCaseGenerator {
    cases: Vec<Vec<String>>,
}

#[async_trait]
impl CaseGenerator for StubCaseGenerator {
    async fn generate_cases(
        &self,
        _request: CaseGenerationRequest,
    ) -> Result<Vec<Vec<String>>, CapTraceError> {
        Ok(self.cases.clone())
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
        action: Action::Write,
        scope: TOKEN_OUT_FILE.to_string(),
    }));
    assert_eq!(summary.required_capabilities.len(), 1);
    assert!(cp_variant.matches_existing_summary == Some(false));

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

#[tokio::test]
async fn matches_existing_command_summaries_for_all_supported_cases() {
    let known_cases: Vec<Vec<&str>> = vec![
        vec!["rm", "-rf", "{{TMP_DIR}}/demo"],
        vec!["cp", "{{IN_FILE}}", "{{OUT_FILE}}"],
        vec!["mv", "{{IN_FILE}}", "{{OUT_FILE}}"],
        vec!["mkdir", "-p", "{{TMP_DIR}}/work"],
        vec!["touch", "-d", "2026-01-01", "{{OUT_FILE}}"],
        vec!["chmod", "-R", "755", "{{OUT_FILE}}", "{{TMP_DIR}}/bin"],
        vec!["chown", "root:root", "{{OUT_FILE}}"],
        vec!["tee", "-a", "{{OUT_FILE}}"],
        vec!["curl", "-X", "POST", "https://api.example.com/v1"],
        vec![
            "curl",
            "-X",
            "POST",
            "https://api.example.com/v1/upload",
            "-d",
            "{\"k\":\"v\"}",
        ],
        vec![
            "curl",
            "https://api.example.com/v1/upload",
            "--data",
            "body",
        ],
        vec![
            "curl",
            "--request",
            "put",
            "--url",
            "https://api.example.com/v1/upload",
            "--data-binary",
            "blob",
        ],
        vec!["ls", "-la"],
    ];

    let unknown_with_summary_cases: Vec<Vec<&str>> = vec![
        vec!["rm", "-rfv", "{{TMP_DIR}}/demo"],
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
    let variant = definition
        .variants
        .iter()
        .find(|variant| variant.argv_template == argv)
        .expect("variant for seed case");

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

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.is_empty() {
                "''".to_string()
            } else if arg.chars().all(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '/' | '.' | ':' | '=' | '@')
            }) {
                arg.clone()
            } else {
                let mut escaped = String::from("'");
                for ch in arg.chars() {
                    if ch == '\'' {
                        escaped.push_str("'\\''");
                    } else {
                        escaped.push(ch);
                    }
                }
                escaped.push('\'');
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
