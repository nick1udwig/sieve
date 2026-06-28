#[derive(Debug, Clone)]
pub(super) struct FlagArg {
    pub(super) argument_name: String,
    pub(super) value_index: usize,
}

#[derive(Debug, Default)]
pub(super) struct ParsedFlags {
    pub(super) saw_help: bool,
    pub(super) dry_run: bool,
    pub(super) params: Option<FlagArg>,
    pub(super) json: Option<FlagArg>,
    pub(super) upload_path: Option<String>,
    pub(super) output_path: Option<String>,
    pub(super) sanitize_template: Option<FlagArg>,
    pub(super) unsupported_flags: Vec<String>,
    pub(super) missing_value_flag: Option<String>,
    pub(super) positionals: Vec<String>,
}

pub(super) fn parse_flags(argv: &[String], start: usize, allow_resolve_refs: bool) -> ParsedFlags {
    let mut out = ParsedFlags::default();
    let mut saw_end_of_flags = false;
    let mut i = start;

    while i < argv.len() {
        let arg = &argv[i];
        if saw_end_of_flags {
            out.positionals.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            saw_end_of_flags = true;
            i += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            out.positionals.push(arg.clone());
            i += 1;
            continue;
        }

        if matches!(arg.as_str(), "-h" | "--help") {
            out.saw_help = true;
            i += 1;
            continue;
        }
        if arg == "--dry-run" {
            out.dry_run = true;
            i += 1;
            continue;
        }
        if arg == "--page-all" {
            i += 1;
            continue;
        }
        if allow_resolve_refs && arg == "--resolve-refs" {
            i += 1;
            continue;
        }

        if let Some(parsed) = parse_flag_value(arg, "-o", "--output", i, argv) {
            handle_string_flag(parsed, &mut out.output_path, &mut out.missing_value_flag);
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--upload", i, argv) {
            handle_string_flag(parsed, &mut out.upload_path, &mut out.missing_value_flag);
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--params", i, argv) {
            handle_tracked_flag(
                parsed,
                "--params",
                &mut out.params,
                &mut out.missing_value_flag,
            );
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--json", i, argv) {
            handle_tracked_flag(parsed, "--json", &mut out.json, &mut out.missing_value_flag);
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = parse_flag_value(arg, "", "--sanitize", i, argv) {
            handle_tracked_flag(
                parsed,
                "--sanitize",
                &mut out.sanitize_template,
                &mut out.missing_value_flag,
            );
            i += consumed_slots(arg);
            continue;
        }
        if let Some(parsed) = consume_ignored_value_flag(arg, i, argv) {
            if let Err(flag) = parsed {
                out.missing_value_flag = Some(flag);
            }
            i += consumed_slots(arg);
            continue;
        }

        out.unsupported_flags.push(arg.clone());
        i += 1;
    }

    out
}

struct ParsedValue {
    value: String,
    value_index: usize,
}

fn parse_flag_value(
    arg: &str,
    short_flag: &str,
    long_flag: &str,
    i: usize,
    argv: &[String],
) -> Option<Result<ParsedValue, String>> {
    if let Some(value) = parse_attached_value(arg, short_flag, long_flag) {
        return Some(if value.is_empty() {
            Err(arg.to_string())
        } else {
            Ok(ParsedValue {
                value: value.to_string(),
                value_index: i,
            })
        });
    }
    if is_flag_name(arg, short_flag, long_flag) {
        return Some(
            argv.get(i + 1)
                .cloned()
                .map(|value| ParsedValue {
                    value,
                    value_index: i + 1,
                })
                .ok_or_else(|| arg.to_string()),
        );
    }
    None
}

fn handle_string_flag(
    parsed: Result<ParsedValue, String>,
    slot: &mut Option<String>,
    missing_value_flag: &mut Option<String>,
) {
    match parsed {
        Ok(parsed) => *slot = Some(parsed.value),
        Err(flag) => *missing_value_flag = Some(flag),
    }
}

fn handle_tracked_flag(
    parsed: Result<ParsedValue, String>,
    argument_name: &str,
    slot: &mut Option<FlagArg>,
    missing_value_flag: &mut Option<String>,
) {
    match parsed {
        Ok(parsed) => {
            *slot = Some(FlagArg {
                argument_name: argument_name.to_string(),
                value_index: parsed.value_index,
            });
        }
        Err(flag) => *missing_value_flag = Some(flag),
    }
}

fn consume_ignored_value_flag(arg: &str, i: usize, argv: &[String]) -> Option<Result<(), String>> {
    const VALUE_FLAGS: &[&str] = &["--format", "--api-version", "--page-limit", "--page-delay"];

    for flag in VALUE_FLAGS {
        if let Some(value) = parse_attached_value(arg, "", flag) {
            return Some(if value.is_empty() {
                Err(arg.to_string())
            } else {
                Ok(())
            });
        }
        if arg == *flag {
            return Some(if argv.get(i + 1).is_none() {
                Err(arg.to_string())
            } else {
                Ok(())
            });
        }
    }

    None
}

fn parse_attached_value<'a>(arg: &'a str, short_flag: &str, long_flag: &str) -> Option<&'a str> {
    if !short_flag.is_empty() && arg.starts_with(short_flag) && arg.len() > short_flag.len() {
        return Some(&arg[short_flag.len()..]);
    }
    if let Some(value) = arg.strip_prefix(&format!("{long_flag}=")) {
        return Some(value);
    }
    None
}

fn is_flag_name(arg: &str, short_flag: &str, long_flag: &str) -> bool {
    (!short_flag.is_empty() && arg == short_flag) || arg == long_flag
}

fn consumed_slots(arg: &str) -> usize {
    if arg.contains('=') || (arg.starts_with("-o") && arg.len() > 2) {
        1
    } else {
        2
    }
}
