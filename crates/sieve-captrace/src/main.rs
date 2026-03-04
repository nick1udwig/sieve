#![forbid(unsafe_code)]

use sieve_captrace::{
    preferred_case_generator_from_env, write_definition_json, BwrapTraceRunner, CapTraceError,
    CapTraceGenerator, CaseGenerator, GenerateDefinitionRequest, GeneratedCommandDefinition,
};
use sieve_quarantine::QuarantineNetworkMode;
use sieve_types::{Action, Resource};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
struct CliArgs {
    command: String,
    seed_shell_cases: Vec<String>,
    include_llm_cases: bool,
    max_llm_cases: usize,
    output_path: Option<PathBuf>,
    rust_output_path: Option<PathBuf>,
    logs_root: PathBuf,
    network_mode: CliNetworkMode,
    allow_write_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliNetworkMode {
    Isolated,
    LocalOnly,
    Full,
}

impl CliNetworkMode {
    fn as_quarantine_mode(self) -> QuarantineNetworkMode {
        match self {
            CliNetworkMode::Isolated => QuarantineNetworkMode::Isolated,
            CliNetworkMode::LocalOnly => QuarantineNetworkMode::LocalOnly,
            CliNetworkMode::Full => QuarantineNetworkMode::Full,
        }
    }
}

#[derive(Debug, Clone)]
struct EscalationHint {
    arg: String,
    reason: String,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("sieve-captrace: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CapTraceError> {
    load_dotenv_if_present()?;
    let args = parse_cli_args(env::args().skip(1))?;
    let trace_runner = Arc::new(BwrapTraceRunner::with_sandbox(
        args.logs_root.clone(),
        args.network_mode.as_quarantine_mode(),
        args.allow_write_paths.clone(),
    ));

    let case_generator: Option<Arc<dyn CaseGenerator>> = if args.include_llm_cases {
        match preferred_case_generator_from_env().await {
            Ok((generator, backend)) => {
                eprintln!("info: case generation backend={}", backend.name());
                Some(generator)
            }
            Err(err) => {
                eprintln!("warning: llm disabled ({err})");
                None
            }
        }
    } else {
        None
    };

    let generator = CapTraceGenerator::new(trace_runner, case_generator);
    let mut definition = generator
        .generate(GenerateDefinitionRequest {
            command: args.command.clone(),
            seed_shell_cases: args.seed_shell_cases.clone(),
            include_llm_cases: args.include_llm_cases,
            max_llm_cases: args.max_llm_cases,
        })
        .await?;
    let hints = collect_escalation_hints(&definition, &args);
    if let Some(first) = hints.first() {
        eprintln!("next: rerun with {}", first.arg);
    }
    for hint in hints {
        eprintln!("hint: {} ({})", hint.arg, hint.reason);
        definition
            .notes
            .push(format!("escalation hint: {} ({})", hint.arg, hint.reason));
    }

    if let Some(output_path) = args.output_path {
        write_definition_json(&output_path, &definition)?;
        println!("{}", output_path.display());
    } else {
        let encoded = serde_json::to_string_pretty(&definition)
            .map_err(|err| CapTraceError::Io(err.to_string()))?;
        println!("{encoded}");
    }

    if let Some(rust_output_path) = args.rust_output_path {
        fs::write(&rust_output_path, &definition.rust_snippet)
            .map_err(|err| CapTraceError::Io(err.to_string()))?;
        println!("{}", rust_output_path.display());
    }

    Ok(())
}

fn load_dotenv_if_present() -> Result<(), CapTraceError> {
    match dotenvy::from_filename(".env") {
        Ok(_) => Ok(()),
        Err(dotenvy::Error::Io(err)) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(CapTraceError::Io(format!("failed to load .env: {err}"))),
    }
}

fn parse_cli_args<I>(iter: I) -> Result<CliArgs, CapTraceError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = iter.into_iter();
    let mut command: Option<String> = None;
    let mut seed_shell_cases = Vec::new();
    let mut include_llm_cases = true;
    let mut max_llm_cases = 4usize;
    let mut output_path = None;
    let mut rust_output_path = None;
    let mut logs_root = std::env::temp_dir().join("sieve-captrace-traces");
    let mut network_mode = CliNetworkMode::Isolated;
    let mut allow_write_paths = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--seed-case" => {
                let value = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --seed-case".to_string())
                })?;
                seed_shell_cases.push(value);
            }
            "--no-llm" => include_llm_cases = false,
            "--max-llm-cases" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --max-llm-cases".to_string())
                })?;
                max_llm_cases = raw.parse::<usize>().map_err(|err| {
                    CapTraceError::Args(format!("invalid --max-llm-cases `{raw}`: {err}"))
                })?;
            }
            "--output" => {
                let raw = args
                    .next()
                    .ok_or_else(|| CapTraceError::Args("missing value for --output".to_string()))?;
                output_path = Some(PathBuf::from(raw));
            }
            "--rust-output" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --rust-output".to_string())
                })?;
                rust_output_path = Some(PathBuf::from(raw));
            }
            "--logs-root" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --logs-root".to_string())
                })?;
                logs_root = PathBuf::from(raw);
            }
            "--allow-local-network" => {
                if network_mode == CliNetworkMode::Full {
                    return Err(CapTraceError::Args(
                        "cannot combine --allow-local-network with --allow-full-network"
                            .to_string(),
                    ));
                }
                network_mode = CliNetworkMode::LocalOnly;
            }
            "--allow-full-network" => {
                if network_mode == CliNetworkMode::LocalOnly {
                    return Err(CapTraceError::Args(
                        "cannot combine --allow-local-network with --allow-full-network"
                            .to_string(),
                    ));
                }
                network_mode = CliNetworkMode::Full;
            }
            "--allow-write" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --allow-write".to_string())
                })?;
                allow_write_paths.push(parse_allow_write_path(&raw)?);
            }
            value if value.starts_with("--") => {
                return Err(CapTraceError::Args(format!("unknown flag `{value}`")));
            }
            value => {
                if command.is_some() {
                    return Err(CapTraceError::Args(format!(
                        "unexpected positional arg `{value}`"
                    )));
                }
                command = Some(value.to_string());
            }
        }
    }

    let command = command
        .ok_or_else(|| CapTraceError::Args("missing required positional <command>".to_string()))?;
    Ok(CliArgs {
        command,
        seed_shell_cases,
        include_llm_cases,
        max_llm_cases,
        output_path,
        rust_output_path,
        logs_root,
        network_mode,
        allow_write_paths,
    })
}

fn parse_allow_write_path(raw: &str) -> Result<PathBuf, CapTraceError> {
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(CapTraceError::Args(format!(
            "--allow-write path must be absolute: `{raw}`"
        )));
    }
    if !path.exists() {
        return Err(CapTraceError::Args(format!(
            "--allow-write path does not exist: `{raw}`"
        )));
    }
    Ok(path)
}

fn collect_escalation_hints(
    definition: &GeneratedCommandDefinition,
    args: &CliArgs,
) -> Vec<EscalationHint> {
    let mut hints = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    let saw_net_attempt = definition.variants.iter().any(|variant| {
        variant
            .attempted_capabilities
            .iter()
            .any(|cap| cap.resource == Resource::Net)
    });
    let saw_unshare_net_failure = definition.variants.iter().any(|variant| {
        variant
            .trace_error
            .as_deref()
            .is_some_and(|err| err.contains("NETLINK_ROUTE socket"))
    });

    if args.network_mode != CliNetworkMode::Full && (saw_net_attempt || saw_unshare_net_failure) {
        if args.network_mode == CliNetworkMode::Isolated {
            push_hint(
                &mut hints,
                &mut seen,
                "--allow-local-network".to_string(),
                "allow loopback-only networking in sandbox".to_string(),
            );
        }
        push_hint(
            &mut hints,
            &mut seen,
            "--allow-full-network".to_string(),
            "allow outbound networking in sandbox".to_string(),
        );
    }

    let blocked_write_scopes = collect_blocked_write_scopes(definition, &args.allow_write_paths);
    if let Some(example) = blocked_write_scopes.first() {
        push_hint(
            &mut hints,
            &mut seen,
            format!("--allow-write {example}"),
            "permit observed filesystem writes outside default writable roots".to_string(),
        );
    }

    hints
}

fn collect_blocked_write_scopes(
    definition: &GeneratedCommandDefinition,
    allow_write_paths: &[PathBuf],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for variant in &definition.variants {
        for capability in &variant.attempted_capabilities {
            if capability.resource != Resource::Fs {
                continue;
            }
            if capability.action != Action::Write && capability.action != Action::Append {
                continue;
            }
            let scope = capability.scope.as_str();
            if scope.is_empty() || scope.starts_with("{{") || scope == "/" {
                continue;
            }
            if is_default_writable_scope(scope) || is_allowed_by_user(scope, allow_write_paths) {
                continue;
            }
            if let Some(example) = writable_hint_path(scope) {
                if seen.insert(example.clone()) {
                    out.push(example);
                }
            }
        }
    }

    out
}

fn is_default_writable_scope(scope: &str) -> bool {
    scope == "/tmp" || scope.starts_with("/tmp/")
}

fn is_allowed_by_user(scope: &str, allow_write_paths: &[PathBuf]) -> bool {
    allow_write_paths.iter().any(|allowed| {
        let allowed = allowed.as_os_str().to_string_lossy();
        scope == allowed || scope.starts_with(&format!("{allowed}/"))
    })
}

fn writable_hint_path(scope: &str) -> Option<String> {
    let path = Path::new(scope);
    if !path.is_absolute() {
        return None;
    }
    let candidate = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    Some(candidate.to_string_lossy().to_string())
}

fn push_hint(
    hints: &mut Vec<EscalationHint>,
    seen: &mut std::collections::BTreeSet<String>,
    arg: String,
    reason: String,
) {
    if seen.insert(arg.clone()) {
        hints.push(EscalationHint { arg, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_captrace::{GeneratedSummaryOutcome, GeneratedVariantDefinition};
    use sieve_types::{Capability, CommandKnowledge};

    #[test]
    fn escalation_hints_suggest_network_flags_when_net_attempt_seen() {
        let definition = GeneratedCommandDefinition {
            schema_version: 1,
            command: "curl".to_string(),
            generated_at_ms: 0,
            variants: vec![variant_with_caps(vec![Capability {
                resource: Resource::Net,
                action: Action::Connect,
                scope: "family=af_inet,address=1.1.1.1,port=443".to_string(),
            }])],
            subcommand_reports: Vec::new(),
            notes: Vec::new(),
            rust_snippet: String::new(),
        };
        let args = cli_args(CliNetworkMode::Isolated, Vec::new());
        let hints = collect_escalation_hints(&definition, &args);
        let args_only: Vec<String> = hints.into_iter().map(|hint| hint.arg).collect();
        assert!(args_only.contains(&"--allow-local-network".to_string()));
        assert!(args_only.contains(&"--allow-full-network".to_string()));
    }

    #[test]
    fn escalation_hints_suggest_allow_write_for_blocked_write_scope() {
        let definition = GeneratedCommandDefinition {
            schema_version: 1,
            command: "tool".to_string(),
            generated_at_ms: 0,
            variants: vec![variant_with_caps(vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/root/.cache/tool/cache.db".to_string(),
            }])],
            subcommand_reports: Vec::new(),
            notes: Vec::new(),
            rust_snippet: String::new(),
        };
        let args = cli_args(CliNetworkMode::Full, Vec::new());
        let hints = collect_escalation_hints(&definition, &args);
        let args_only: Vec<String> = hints.into_iter().map(|hint| hint.arg).collect();
        assert!(args_only.contains(&"--allow-write /root/.cache/tool".to_string()));
    }

    #[test]
    fn escalation_hints_skip_allow_write_when_scope_already_allowed() {
        let definition = GeneratedCommandDefinition {
            schema_version: 1,
            command: "tool".to_string(),
            generated_at_ms: 0,
            variants: vec![variant_with_caps(vec![Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/root/.cache/tool/cache.db".to_string(),
            }])],
            subcommand_reports: Vec::new(),
            notes: Vec::new(),
            rust_snippet: String::new(),
        };
        let args = cli_args(
            CliNetworkMode::Full,
            vec![PathBuf::from("/root/.cache/tool")],
        );
        let hints = collect_escalation_hints(&definition, &args);
        assert!(hints
            .into_iter()
            .all(|hint| !hint.arg.starts_with("--allow-write")));
    }

    fn cli_args(network_mode: CliNetworkMode, allow_write_paths: Vec<PathBuf>) -> CliArgs {
        CliArgs {
            command: "tool".to_string(),
            seed_shell_cases: Vec::new(),
            include_llm_cases: false,
            max_llm_cases: 1,
            output_path: None,
            rust_output_path: None,
            logs_root: PathBuf::from("/tmp"),
            network_mode,
            allow_write_paths,
        }
    }

    fn variant_with_caps(caps: Vec<Capability>) -> GeneratedVariantDefinition {
        GeneratedVariantDefinition {
            case_id: "case".to_string(),
            command_path: Vec::new(),
            argv_template: vec!["tool".to_string()],
            argv_effective: vec!["tool".to_string()],
            trace_path: None,
            exit_code: Some(1),
            attempted_capabilities: caps,
            trace_derived_summary: None,
            summary_outcome: GeneratedSummaryOutcome {
                knowledge: CommandKnowledge::Unknown,
                summary: None,
                reason: None,
            },
            matches_existing_summary: None,
            trace_error: None,
        }
    }
}

fn print_usage() {
    let usage = "\
Usage:
  sieve-captrace <command> [--seed-case '<shell cmd>']... [--no-llm]
                [--max-llm-cases <N>] [--output <path>]
                [--rust-output <path>] [--logs-root <path>]
                [--allow-local-network|--allow-full-network]
                [--allow-write <absolute path>]...

Examples:
  sieve-captrace mkdir --seed-case 'mkdir -p {{TMP_DIR}}/logs' --rust-output /tmp/mkdir.rs
  sieve-captrace cp --output /tmp/cp-definition.json
  sieve-captrace curl --allow-full-network --allow-write /root/.cache --output /tmp/curl-definition.json
";
    println!("{usage}");
}
