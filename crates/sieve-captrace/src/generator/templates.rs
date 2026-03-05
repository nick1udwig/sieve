use crate::command_match::args_after_command;
use crate::fixture::{
    TOKEN_ARG, TOKEN_DATA, TOKEN_HEADER, TOKEN_IN_FILE, TOKEN_IN_FILE_2, TOKEN_KV, TOKEN_OUT_FILE,
    TOKEN_TMP_DIR, TOKEN_URL,
};

pub(super) const TEMPLATE_TOKEN_FILES: [&str; 4] = [
    TOKEN_TMP_DIR,
    TOKEN_IN_FILE,
    TOKEN_IN_FILE_2,
    TOKEN_OUT_FILE,
];
pub(super) const TEMPLATE_TOKEN_GENERICS: [&str; 5] =
    [TOKEN_URL, TOKEN_HEADER, TOKEN_DATA, TOKEN_KV, TOKEN_ARG];

pub(super) fn abstract_argv_template(
    argv_template: &[String],
    command_path: &[String],
) -> Vec<String> {
    let mut out = Vec::with_capacity(argv_template.len());
    let mut value_kind_from_prev_flag: Option<&str> = None;
    let prefix_len = 1 + command_path.len();

    for (index, arg) in argv_template.iter().enumerate() {
        if index == 0 || index < prefix_len {
            out.push(arg.clone());
            continue;
        }

        if is_template_token(arg) {
            out.push(arg.clone());
            value_kind_from_prev_flag = None;
            continue;
        }
        if contains_template_token(arg) {
            out.push(arg.clone());
            value_kind_from_prev_flag = None;
            continue;
        }

        if let Some(kind) = value_kind_from_prev_flag.take() {
            out.push(placeholder_for_value_kind(kind).to_string());
            continue;
        }

        if let Some(kind) = value_kind_for_separate_flag(arg) {
            out.push(arg.clone());
            value_kind_from_prev_flag = Some(kind);
            continue;
        }

        if arg.starts_with('-') {
            if let Some((flag, value)) = split_flag_value(arg) {
                if let Some(kind) = value_kind_for_inline_flag(flag) {
                    out.push(format!("{flag}={}", placeholder_for_value_kind(kind)));
                } else if is_kv_like(value) {
                    out.push(format!("{flag}={TOKEN_KV}"));
                } else {
                    out.push(format!("{flag}={TOKEN_ARG}"));
                }
            } else {
                out.push(arg.clone());
            }
            continue;
        }

        out.push(abstract_positional_value(arg).to_string());
    }

    out
}

pub(super) fn infer_command_path(
    argv_template: &[String],
    command: &str,
    known_command_paths: &[Vec<String>],
) -> Vec<String> {
    let args_after_command = args_after_command(argv_template, command);
    if args_after_command.is_empty() {
        return Vec::new();
    }

    let mut best_match: Option<Vec<String>> = None;
    for known_path in known_command_paths {
        if known_path.is_empty() || known_path.len() > args_after_command.len() {
            continue;
        }
        if args_after_command
            .iter()
            .take(known_path.len())
            .eq(known_path.iter())
        {
            if best_match
                .as_ref()
                .is_none_or(|existing| known_path.len() > existing.len())
            {
                best_match = Some(known_path.clone());
            }
        }
    }
    if let Some(best_match) = best_match {
        return best_match;
    }

    let mut inferred = Vec::new();
    for token in args_after_command {
        if token.starts_with('-') || token.starts_with("{{") {
            break;
        }
        if !is_subcommand_token(token) {
            break;
        }
        inferred.push(token.clone());
        if inferred.len() >= 3 {
            break;
        }
    }
    inferred
}

pub(super) fn is_template_token(value: &str) -> bool {
    TEMPLATE_TOKEN_FILES
        .iter()
        .chain(TEMPLATE_TOKEN_GENERICS.iter())
        .any(|token| value == *token)
}

pub(super) fn contains_template_token(value: &str) -> bool {
    TEMPLATE_TOKEN_FILES
        .iter()
        .chain(TEMPLATE_TOKEN_GENERICS.iter())
        .any(|token| value.contains(token))
}

pub(super) fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

pub(super) fn is_subcommand_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn value_kind_for_separate_flag(flag: &str) -> Option<&'static str> {
    match flag {
        "-H" | "--header" => Some("header"),
        "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--data-urlencode"
        | "--json" => Some("data"),
        "--param" => Some("kv"),
        "--url" | "--goggle" => Some("url"),
        "--request" | "-X" => Some("arg"),
        _ => {
            if is_file_like_flag(flag) {
                Some("file")
            } else if is_url_like_flag(flag) {
                Some("url")
            } else {
                None
            }
        }
    }
}

fn value_kind_for_inline_flag(flag: &str) -> Option<&'static str> {
    match flag {
        "--header" => Some("header"),
        "--data" | "--data-raw" | "--data-binary" | "--data-ascii" | "--data-urlencode"
        | "--json" => Some("data"),
        "--param" => Some("kv"),
        "--url" | "--goggle" => Some("url"),
        _ => {
            if is_file_like_flag(flag) {
                Some("file")
            } else if is_url_like_flag(flag) {
                Some("url")
            } else {
                None
            }
        }
    }
}

fn is_file_like_flag(flag: &str) -> bool {
    let lowered = flag.to_ascii_lowercase();
    lowered.contains("file")
        || lowered.contains("path")
        || lowered.contains("config")
        || lowered == "-o"
        || lowered == "--output"
}

fn is_url_like_flag(flag: &str) -> bool {
    let lowered = flag.to_ascii_lowercase();
    lowered.contains("url") || lowered.contains("endpoint")
}

fn placeholder_for_value_kind(kind: &str) -> &'static str {
    match kind {
        "header" => TOKEN_HEADER,
        "data" => TOKEN_DATA,
        "kv" => TOKEN_KV,
        "url" => TOKEN_URL,
        "file" => TOKEN_IN_FILE,
        _ => TOKEN_ARG,
    }
}

fn abstract_positional_value(value: &str) -> &'static str {
    if looks_like_url(value) {
        return TOKEN_URL;
    }
    if is_kv_like(value) {
        return TOKEN_KV;
    }
    TOKEN_ARG
}

fn is_kv_like(value: &str) -> bool {
    value.contains('=') && !value.starts_with('-')
}

fn split_flag_value(flag: &str) -> Option<(&str, &str)> {
    let eq = flag.find('=')?;
    Some((&flag[..eq], &flag[eq + 1..]))
}
