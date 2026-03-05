use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::SummaryOutcome;

pub(super) fn summarize_tts(argv: &[String]) -> SummaryOutcome {
    let mut text_path: Option<String> = None;
    let mut has_txt_input = false;
    let mut output_path: Option<String> = None;
    let mut unsupported_flags = Vec::new();
    let mut missing_value_flag: Option<String> = None;
    let mut saw_end_of_flags = false;
    let mut i = 2usize;

    while i < argv.len() {
        let arg = argv[i].as_str();
        if saw_end_of_flags {
            if text_path.is_none() {
                text_path = Some(argv[i].clone());
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
            if text_path.is_none() {
                text_path = Some(argv[i].clone());
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
        if let Some(value) = super::parse_value_flag_inline(arg, "-t", "--txt") {
            if value.is_empty() {
                missing_value_flag = Some(argv[i].clone());
                break;
            }
            has_txt_input = true;
            i += 1;
            continue;
        }
        if matches!(arg, "-t" | "--txt") {
            if argv.get(i + 1).is_none() {
                missing_value_flag = Some(argv[i].clone());
                break;
            }
            has_txt_input = true;
            i += 2;
            continue;
        }
        if parse_ignored_tts_flag(arg, &mut i, argv, &mut missing_value_flag) {
            continue;
        }
        unsupported_flags.push(argv[i].clone());
        i += 1;
    }

    if let Some(flag) = missing_value_flag {
        return super::super::unknown_outcome(&format!("st tts flag missing value: {flag}"));
    }
    if !unsupported_flags.is_empty() {
        return super::super::unknown_with_flags("unsupported st tts flags", unsupported_flags);
    }
    if text_path.is_none() && !has_txt_input {
        return super::super::unknown_outcome("st tts missing text input");
    }

    let mut required_capabilities = vec![Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: super::OPENAI_CONNECT_SCOPE.to_string(),
    }];
    if let Some(path) = text_path {
        required_capabilities.push(Capability {
            resource: Resource::Fs,
            action: Action::Read,
            scope: path,
        });
    }
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

fn parse_ignored_tts_flag(
    arg: &str,
    i: &mut usize,
    argv: &[String],
    missing_value_flag: &mut Option<String>,
) -> bool {
    super::parse_ignored_value_flag(
        arg,
        i,
        argv,
        missing_value_flag,
        &[
            "--config",
            "--provider",
            "--format",
            "--instructions",
            "--model",
            "--speed",
            "--voice",
        ],
    )
}
