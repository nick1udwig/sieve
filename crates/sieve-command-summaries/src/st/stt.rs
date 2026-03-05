use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::SummaryOutcome;

pub(super) fn summarize_stt(argv: &[String]) -> SummaryOutcome {
    let mut input_path: Option<String> = None;
    let mut output_path: Option<String> = None;
    let mut unsupported_flags = Vec::new();
    let mut missing_value_flag: Option<String> = None;
    let mut saw_end_of_flags = false;
    let mut i = 2usize;

    while i < argv.len() {
        let arg = argv[i].as_str();
        if saw_end_of_flags {
            if input_path.is_none() {
                input_path = Some(argv[i].clone());
            }
            i += 1;
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }
        if arg == "-h" || arg == "--help" {
            return super::known_empty_outcome();
        }
        if !arg.starts_with('-') || arg == "-" {
            if input_path.is_none() {
                input_path = Some(argv[i].clone());
            }
            i += 1;
            continue;
        }
        if let Some(value) = super::parse_value_flag_inline(arg, "-o", "--output") {
            if value.is_empty() {
                missing_value_flag = Some(argv[i].clone());
                break;
            }
            output_path = Some(value.to_string());
            i += 1;
            continue;
        }
        if matches!(arg, "-o" | "--output") {
            let Some(value) = argv.get(i + 1) else {
                missing_value_flag = Some(argv[i].clone());
                break;
            };
            output_path = Some(value.clone());
            i += 2;
            continue;
        }
        if parse_ignored_stt_flag(arg, &mut i, argv, &mut missing_value_flag) {
            continue;
        }
        unsupported_flags.push(argv[i].clone());
        i += 1;
    }

    if let Some(flag) = missing_value_flag {
        return super::super::unknown_outcome(&format!("st stt flag missing value: {flag}"));
    }
    if !unsupported_flags.is_empty() {
        return super::super::unknown_with_flags("unsupported st stt flags", unsupported_flags);
    }
    let Some(input_path) = input_path else {
        return super::super::unknown_outcome("st stt missing audio file");
    };

    let mut required_capabilities = vec![
        Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: input_path,
        },
        Capability {
            resource: Resource::Net,
            action: Action::Connect,
            scope: super::OPENAI_CONNECT_SCOPE.to_string(),
        },
    ];
    if let Some(path) = output_path {
        required_capabilities.push(Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: path,
        });
    }
    super::super::known_outcome(CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn parse_ignored_stt_flag(
    arg: &str,
    i: &mut usize,
    argv: &[String],
    missing_value_flag: &mut Option<String>,
) -> bool {
    if matches!(arg, "--include-logprobs" | "--stream") {
        *i += 1;
        return true;
    }
    super::parse_ignored_value_flag(
        arg,
        i,
        argv,
        missing_value_flag,
        &[
            "--config",
            "--provider",
            "--language",
            "--model",
            "--prompt",
            "--response-format",
            "--temperature",
        ],
    )
}
