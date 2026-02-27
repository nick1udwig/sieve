#![forbid(unsafe_code)]

use sieve_captrace::{
    write_definition_json, BwrapTraceRunner, CapTraceError, CapTraceGenerator, CaseGenerator,
    GenerateDefinitionRequest, PlannerCaseGenerator,
};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
struct CliArgs {
    command: String,
    seed_shell_cases: Vec<String>,
    include_llm_cases: bool,
    max_llm_cases: usize,
    output_path: Option<PathBuf>,
    logs_root: PathBuf,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("sieve-captrace: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), CapTraceError> {
    let args = parse_cli_args(env::args().skip(1))?;
    let trace_runner = Arc::new(BwrapTraceRunner::new(args.logs_root));

    let case_generator: Option<Arc<dyn CaseGenerator>> = if args.include_llm_cases {
        match PlannerCaseGenerator::from_env() {
            Ok(generator) => Some(Arc::new(generator)),
            Err(err) => {
                eprintln!("warning: llm disabled ({err})");
                None
            }
        }
    } else {
        None
    };

    let generator = CapTraceGenerator::new(trace_runner, case_generator);
    let definition = generator
        .generate(GenerateDefinitionRequest {
            command: args.command,
            seed_shell_cases: args.seed_shell_cases,
            include_llm_cases: args.include_llm_cases,
            max_llm_cases: args.max_llm_cases,
        })
        .await?;

    if let Some(output_path) = args.output_path {
        write_definition_json(&output_path, &definition)?;
        println!("{}", output_path.display());
    } else {
        let encoded = serde_json::to_string_pretty(&definition)
            .map_err(|err| CapTraceError::Io(err.to_string()))?;
        println!("{encoded}");
    }

    Ok(())
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
    let mut logs_root = std::env::temp_dir().join("sieve-captrace-traces");

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
            "--logs-root" => {
                let raw = args.next().ok_or_else(|| {
                    CapTraceError::Args("missing value for --logs-root".to_string())
                })?;
                logs_root = PathBuf::from(raw);
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
        logs_root,
    })
}

fn print_usage() {
    let usage = "\
Usage:
  sieve-captrace <command> [--seed-case '<shell cmd>']... [--no-llm]
                [--max-llm-cases <N>] [--output <path>] [--logs-root <path>]

Examples:
  sieve-captrace mkdir --seed-case 'mkdir -p {{TMP_DIR}}/logs'
  sieve-captrace cp --output /tmp/cp-definition.json
";
    println!("{usage}");
}
