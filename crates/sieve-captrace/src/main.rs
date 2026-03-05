#![forbid(unsafe_code)]

mod cli;
mod escalation;

use crate::cli::{load_dotenv_if_present, parse_cli_args};
use crate::escalation::collect_escalation_hints;
use sieve_captrace::{
    preferred_case_generator_from_env, write_definition_json, BwrapTraceRunner, CapTraceError,
    CapTraceGenerator, CaseGenerator, GenerateDefinitionRequest,
};
use std::env;
use std::fs;
use std::sync::Arc;

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
