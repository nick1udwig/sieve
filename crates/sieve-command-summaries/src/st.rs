use sieve_types::{Action, Capability, CommandSummary, Resource};

use crate::SummaryOutcome;

const OPENAI_CONNECT_SCOPE: &str = "https://api.openai.com/";

pub(super) fn summarize_st(argv: &[String]) -> Option<SummaryOutcome> {
    let inner = super::strip_sudo(argv);
    if !super::basename(inner.first()).is_some_and(|cmd| cmd == "st") {
        return None;
    }
    if inner.len() < 2 {
        return Some(super::unknown_outcome("st missing subcommand"));
    }

    let subcommand = inner[1].as_str();
    if matches!(subcommand, "help" | "providers" | "completion") {
        return Some(super::known_outcome(CommandSummary {
            required_capabilities: Vec::new(),
            sink_checks: Vec::new(),
            unsupported_flags: Vec::new(),
        }));
    }

    match subcommand {
        "stt" => Some(summarize_stt(inner)),
        "tts" => Some(summarize_tts(inner)),
        "config" => Some(super::unknown_outcome("unsupported st subcommand: config")),
        other => Some(super::unknown_outcome(&format!(
            "unsupported st subcommand: {other}"
        ))),
    }
}

fn summarize_stt(argv: &[String]) -> SummaryOutcome {
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
            return super::known_outcome(CommandSummary {
                required_capabilities: Vec::new(),
                sink_checks: Vec::new(),
                unsupported_flags: Vec::new(),
            });
        }
        if !arg.starts_with('-') || arg == "-" {
            if input_path.is_none() {
                input_path = Some(argv[i].clone());
            }
            i += 1;
            continue;
        }
        if let Some(value) = parse_value_flag_inline(arg, "-o", "--output") {
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
        return super::unknown_outcome(&format!("st stt flag missing value: {flag}"));
    }
    if !unsupported_flags.is_empty() {
        return super::unknown_with_flags("unsupported st stt flags", unsupported_flags);
    }
    let Some(input_path) = input_path else {
        return super::unknown_outcome("st stt missing audio file");
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
            scope: OPENAI_CONNECT_SCOPE.to_string(),
        },
    ];
    if let Some(path) = output_path {
        required_capabilities.push(Capability {
            resource: Resource::Fs,
            action: Action::Write,
            scope: path,
        });
    }
    super::known_outcome(CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn summarize_tts(argv: &[String]) -> SummaryOutcome {
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
            return super::known_outcome(CommandSummary {
                required_capabilities: Vec::new(),
                sink_checks: Vec::new(),
                unsupported_flags: Vec::new(),
            });
        }
        if !arg.starts_with('-') || arg == "-" {
            if text_path.is_none() {
                text_path = Some(argv[i].clone());
            }
            i += 1;
            continue;
        }
        if let Some(value) = parse_value_flag_inline(arg, "-o", "--output") {
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
        if let Some(value) = parse_value_flag_inline(arg, "-t", "--txt") {
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
        return super::unknown_outcome(&format!("st tts flag missing value: {flag}"));
    }
    if !unsupported_flags.is_empty() {
        return super::unknown_with_flags("unsupported st tts flags", unsupported_flags);
    }
    if text_path.is_none() && !has_txt_input {
        return super::unknown_outcome("st tts missing text input");
    }

    let mut required_capabilities = vec![Capability {
        resource: Resource::Net,
        action: Action::Connect,
        scope: OPENAI_CONNECT_SCOPE.to_string(),
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
    super::known_outcome(CommandSummary {
        required_capabilities,
        sink_checks: Vec::new(),
        unsupported_flags: Vec::new(),
    })
}

fn parse_value_flag_inline<'a>(arg: &'a str, short: &str, long: &str) -> Option<&'a str> {
    if arg.starts_with(short) && arg.len() > short.len() {
        return Some(&arg[short.len()..]);
    }
    arg.strip_prefix(&format!("{long}="))
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
    parse_ignored_value_flag(
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

fn parse_ignored_tts_flag(
    arg: &str,
    i: &mut usize,
    argv: &[String],
    missing_value_flag: &mut Option<String>,
) -> bool {
    parse_ignored_value_flag(
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

fn parse_ignored_value_flag(
    arg: &str,
    i: &mut usize,
    argv: &[String],
    missing_value_flag: &mut Option<String>,
    long_flags: &[&str],
) -> bool {
    for long_flag in long_flags {
        if let Some(value) = arg.strip_prefix(&format!("{long_flag}=")) {
            if value.is_empty() {
                *missing_value_flag = Some(arg.to_string());
            }
            *i += 1;
            return true;
        }
        if arg == *long_flag {
            if argv.get(*i + 1).is_none() {
                *missing_value_flag = Some(arg.to_string());
                *i += 1;
            } else {
                *i += 2;
            }
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use sieve_types::CommandKnowledge;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn st_stt_audio_file_is_known_with_read_and_connect() {
        let out = summarize_st(&argv(&["st", "stt", "/tmp/input.ogg"])).expect("st summary");
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("summary");
        assert_eq!(
            summary.required_capabilities,
            vec![
                Capability {
                    resource: Resource::Fs,
                    action: Action::Read,
                    scope: "/tmp/input.ogg".to_string(),
                },
                Capability {
                    resource: Resource::Net,
                    action: Action::Connect,
                    scope: OPENAI_CONNECT_SCOPE.to_string(),
                }
            ]
        );
    }

    #[test]
    fn st_stt_output_path_adds_fs_write() {
        let out = summarize_st(&argv(&[
            "st",
            "stt",
            "/tmp/input.ogg",
            "--output",
            "/tmp/out.txt",
        ]))
        .expect("st summary");
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("summary");
        assert_eq!(
            summary.required_capabilities[2],
            Capability {
                resource: Resource::Fs,
                action: Action::Write,
                scope: "/tmp/out.txt".to_string(),
            }
        );
    }

    #[test]
    fn st_tts_text_file_and_output_is_known() {
        let out = summarize_st(&argv(&[
            "st",
            "tts",
            "/tmp/input.txt",
            "--format",
            "ogg",
            "--output",
            "/tmp/out.ogg",
        ]))
        .expect("st summary");
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("summary");
        assert_eq!(
            summary.required_capabilities,
            vec![
                Capability {
                    resource: Resource::Net,
                    action: Action::Connect,
                    scope: OPENAI_CONNECT_SCOPE.to_string(),
                },
                Capability {
                    resource: Resource::Fs,
                    action: Action::Read,
                    scope: "/tmp/input.txt".to_string(),
                },
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "/tmp/out.ogg".to_string(),
                }
            ]
        );
    }

    #[test]
    fn st_tts_txt_input_is_known_without_fs_read() {
        let out = summarize_st(&argv(&[
            "st",
            "tts",
            "--txt",
            "hello",
            "--output",
            "/tmp/out.ogg",
        ]))
        .expect("st summary");
        assert_eq!(out.knowledge, CommandKnowledge::Known);
        let summary = out.summary.expect("summary");
        assert_eq!(
            summary.required_capabilities,
            vec![
                Capability {
                    resource: Resource::Net,
                    action: Action::Connect,
                    scope: OPENAI_CONNECT_SCOPE.to_string(),
                },
                Capability {
                    resource: Resource::Fs,
                    action: Action::Write,
                    scope: "/tmp/out.ogg".to_string(),
                }
            ]
        );
    }

    #[test]
    fn st_tts_missing_input_is_unknown() {
        let out =
            summarize_st(&argv(&["st", "tts", "--output", "/tmp/out.ogg"])).expect("st summary");
        assert_eq!(out.knowledge, CommandKnowledge::Unknown);
        assert_eq!(out.reason.as_deref(), Some("st tts missing text input"));
    }
}
