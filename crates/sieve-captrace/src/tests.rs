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
    assert!(!summary
        .required_capabilities
        .iter()
        .any(|cap| cap.scope == "/etc/ld.so.cache"));
}
