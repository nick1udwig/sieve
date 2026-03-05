mod stt;
#[cfg(test)]
mod tests;
mod tts;

use sieve_types::CommandSummary;

use crate::SummaryOutcome;

const OPENAI_CONNECT_SCOPE: &str = "https://api.openai.com/";
#[cfg(test)]
pub(crate) const PLANNER_STT_EXAMPLE: &str = "st stt <audio-file>";
#[cfg(test)]
pub(crate) const PLANNER_TTS_FILE_EXAMPLE: &str =
    "st tts <text-file> --format opus --output <audio-file>";
#[cfg(test)]
pub(crate) const PLANNER_TTS_INLINE_EXAMPLE: &str =
    "st tts --txt \"...\" --format opus --output <audio-file>";
pub(crate) const PLANNER_CATALOG_DESCRIPTION: &str = "Speech CLI for transcription and synthesis. STT pattern: `st stt <audio-file>` (prints transcript to stdout, optionally `-o <file>`). TTS pattern: `st tts <text-file> --format opus --output <audio-file>` or `st tts --txt \"...\" --format opus --output <audio-file>`.";

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
        return Some(known_empty_outcome());
    }

    match subcommand {
        "stt" => Some(stt::summarize_stt(inner)),
        "tts" => Some(tts::summarize_tts(inner)),
        "config" => Some(super::unknown_outcome("unsupported st subcommand: config")),
        other => Some(super::unknown_outcome(&format!(
            "unsupported st subcommand: {other}"
        ))),
    }
}

pub(super) fn known_empty_outcome() -> SummaryOutcome {
    super::known_outcome(CommandSummary {
        required_capabilities: Vec::new(),
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
