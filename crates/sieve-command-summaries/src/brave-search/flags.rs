#[derive(Debug, Default)]
pub(super) struct ParsedFlags {
    pub(super) unsupported_flags: Vec<String>,
    pub(super) positionals: Vec<String>,
    values: Vec<(String, String)>,
    pub(super) saw_help: bool,
    pub(super) missing_value_flag: Option<String>,
}

pub(super) fn parse_flags(
    argv: &[String],
    start: usize,
    value_flags: &[&str],
    bool_flags: &[&str],
) -> ParsedFlags {
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

        let Some((name, inline_value)) = parse_flag_token(arg) else {
            out.positionals.push(arg.clone());
            i += 1;
            continue;
        };

        if matches!(name, "h" | "help") {
            out.saw_help = true;
            i += 1;
            continue;
        }

        if bool_flags.contains(&name) {
            i += 1;
            continue;
        }

        if value_flags.contains(&name) {
            if let Some(value) = inline_value {
                if value.is_empty() {
                    out.missing_value_flag = Some(arg.clone());
                    break;
                }
                out.values.push((name.to_string(), value.to_string()));
                i += 1;
                continue;
            }
            if i + 1 >= argv.len() {
                out.missing_value_flag = Some(arg.clone());
                break;
            }
            out.values.push((name.to_string(), argv[i + 1].clone()));
            i += 2;
            continue;
        }

        out.unsupported_flags.push(arg.clone());
        i += 1;
    }

    out
}

pub(super) fn latest_value(parsed: &ParsedFlags, key: &str, default: &str) -> String {
    latest_optional_value(parsed, key).unwrap_or_else(|| default.to_string())
}

pub(super) fn latest_optional_value(parsed: &ParsedFlags, key: &str) -> Option<String> {
    parsed
        .values
        .iter()
        .rev()
        .find_map(|(k, v)| if k == key { Some(v.clone()) } else { None })
}

fn parse_flag_token(arg: &str) -> Option<(&str, Option<&str>)> {
    if !arg.starts_with('-') || arg == "-" {
        return None;
    }

    let trimmed = arg.trim_start_matches('-');
    if trimmed.is_empty() {
        return None;
    }

    if let Some((name, value)) = trimmed.split_once('=') {
        return Some((name, Some(value)));
    }
    Some((trimmed, None))
}
