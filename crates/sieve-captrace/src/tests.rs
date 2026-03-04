#![forbid(unsafe_code)]

use crate::fixture::{
    TOKEN_ARG, TOKEN_IN_FILE, TOKEN_KV, TOKEN_OUT_FILE, TOKEN_TMP_DIR, TOKEN_URL,
};
use crate::generator::{
    render_rust_snippet, CapTraceGenerator, GenerateDefinitionRequest, TraceRequest, TraceRunner,
};
use crate::planner::{CaseGenerationRequest, CaseGenerator};
use crate::CapTraceError;
use async_trait::async_trait;
use sieve_command_summaries::{CommandSummarizer, DefaultCommandSummarizer};
use sieve_types::{Action, Capability, CommandKnowledge, QuarantineReport, Resource, RunId};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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

struct CurlNetTraceRunner;

#[async_trait]
impl TraceRunner for CurlNetTraceRunner {
    async fn trace(&self, request: TraceRequest) -> Result<QuarantineReport, CapTraceError> {
        let attempted_capabilities = if request.argv.first().map(String::as_str) == Some("curl") {
            vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "family=af_inet,address=example.com,port=443".to_string(),
            }]
        } else {
            Vec::new()
        };

        Ok(QuarantineReport {
            run_id: RunId(request.run_id),
            trace_path: "/tmp/stub-trace-curl".to_string(),
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

fn write_fake_help_script(path: &PathBuf) {
    let script = r#"#!/usr/bin/env bash
if [ "$1" = "--help" ]; then
cat <<'EOF'
bravesearch - Simple Brave Search CLI

Usage:
  bravesearch <command> [options]

Commands:
  search [query]      Run Brave web search
  news                Run Brave news search
  version             Print version
  help                Show this help
EOF
exit 0
fi

if [ "$1" = "search" ] && [ "$2" = "--help" ]; then
cat <<'EOF'
Usage:
  bravesearch search [flags] [query]

Flags:
  --q, --query string    Search query
  --count int            Number of results
EOF
exit 0
fi

if [ "$1" = "news" ] && [ "$2" = "--help" ]; then
cat <<'EOF'
Usage:
  bravesearch news [flags] [query]

Flags:
  --count int            Number of results
EOF
exit 0
fi

exit 1
"#
    .to_string();
    fs::write(path, script).expect("write fake command script");
    let mut permissions = fs::metadata(path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod fake command script");
}

fn write_failing_script(path: &PathBuf) {
    let script = r#"#!/usr/bin/env bash
exit 1
"#
    .to_string();
    fs::write(path, script).expect("write failing command script");
    let mut permissions = fs::metadata(path).expect("script metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod failing command script");
}

fn make_temp_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "sieve-captrace-test-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}
