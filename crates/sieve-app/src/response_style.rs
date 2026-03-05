use crate::{url_is_likely_asset, ResponseTurnInput};
use std::collections::BTreeSet;

pub(crate) fn dedupe_preserve_order(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn trim_url_candidate(candidate: &str) -> &str {
    let mut end = candidate.len();
    while end > 0 {
        let Some(ch) = candidate[..end].chars().next_back() else {
            break;
        };
        if matches!(
            ch,
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\'' | '`'
        ) {
            end = end.saturating_sub(ch.len_utf8());
            continue;
        }
        break;
    }
    &candidate[..end]
}

pub(crate) fn extract_plain_urls_from_text(message: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    while cursor < message.len() {
        let remaining = &message[cursor..];
        let http_pos = remaining.find("http://");
        let https_pos = remaining.find("https://");
        let Some(rel_start) = (match (http_pos, https_pos) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }) else {
            break;
        };
        let start = cursor + rel_start;
        let mut end = message.len();
        for (offset, ch) in message[start..].char_indices() {
            if offset == 0 {
                continue;
            }
            if ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | '\\' | '`') {
                end = start + offset;
                break;
            }
        }
        let candidate = trim_url_candidate(&message[start..end]);
        if candidate.starts_with("https://") || candidate.starts_with("http://") {
            urls.push(candidate.to_string());
        }
        cursor = end.max(start.saturating_add(1));
    }
    dedupe_preserve_order(urls)
}

pub(crate) fn filter_non_asset_urls(urls: Vec<String>) -> Vec<String> {
    dedupe_preserve_order(
        urls.into_iter()
            .filter(|url| !url_is_likely_asset(url))
            .collect(),
    )
}

pub(crate) fn strip_asset_urls_from_message(message: &str) -> String {
    let mut sanitized = message.to_string();
    for url in extract_plain_urls_from_text(message) {
        if url_is_likely_asset(&url) {
            sanitized = sanitized.replace(&url, "");
        }
    }
    let mut lines = Vec::new();
    for line in sanitized.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    lines.join("\n")
}

fn contains_plain_url(message: &str) -> bool {
    message.contains("https://") || message.contains("http://")
}

fn contains_linkish_placeholder(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "provided link",
        "provided links",
        "provided url",
        "provided urls",
        "full results",
        "search results",
        "source above",
        "source below",
        "sources above",
        "sources below",
        "url above",
        "url below",
        "urls above",
        "urls below",
        "link above",
        "link below",
        "links above",
        "links below",
    ]
    .iter()
    .any(|pattern| normalized.contains(pattern))
}

fn mentions_linkish_text(message: &str) -> bool {
    contains_linkish_placeholder(message)
}

fn split_sentences(message: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in message.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
            }
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn remove_linkish_sentences(message: &str) -> String {
    let kept: Vec<String> = split_sentences(message)
        .into_iter()
        .filter(|sentence| !contains_linkish_placeholder(sentence))
        .collect();
    if kept.is_empty() {
        message
            .replace("provided link", "")
            .replace("provided links", "")
            .replace("provided url", "")
            .replace("provided urls", "")
            .trim()
            .to_string()
    } else {
        kept.join(" ")
    }
}

pub(crate) fn enforce_link_policy(
    message: String,
    source_urls: &[String],
    trusted_user_message: &str,
) -> String {
    if !mentions_linkish_text(&message) || contains_plain_url(&message) {
        return message;
    }
    if !source_urls.is_empty() && user_requested_sources(trusted_user_message) {
        let mut out = message.trim().to_string();
        if !out.is_empty() {
            out.push('\n');
        }
        for url in source_urls.iter().take(3) {
            out.push_str(url);
            out.push('\n');
        }
        return out.trim().to_string();
    }
    remove_linkish_sentences(&message)
}

fn normalized_words(input: &str) -> String {
    input
        .to_ascii_lowercase()
        .replace(
            ['?', '!', '.', ',', ';', ':', '(', ')', '[', ']', '{', '}'],
            " ",
        )
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn user_requested_sources(trusted_user_message: &str) -> bool {
    let normalized = normalized_words(trusted_user_message);
    normalized.contains("source")
        || normalized.contains("sources")
        || normalized.contains("link")
        || normalized.contains("links")
        || normalized.contains("url")
        || normalized.contains("citation")
        || normalized.contains("citations")
        || normalized.contains("reference")
        || normalized.contains("references")
}

pub(crate) fn user_requested_detailed_output(trusted_user_message: &str) -> bool {
    let normalized = normalized_words(trusted_user_message);
    normalized.contains("detailed")
        || normalized.contains("in detail")
        || normalized.contains("step by step")
        || normalized.contains("full breakdown")
        || normalized.contains("thorough")
        || normalized.contains("comprehensive")
        || normalized.contains("long form")
        || normalized.contains("explain")
}

fn sentence_like_count(message: &str) -> usize {
    split_sentences(message)
        .into_iter()
        .filter(|sentence| !sentence.trim().is_empty())
        .count()
}

pub(crate) fn concise_style_diagnostic(
    composed_message: &str,
    trusted_user_message: &str,
) -> Option<String> {
    if user_requested_detailed_output(trusted_user_message) {
        return None;
    }
    let sentence_count = sentence_like_count(composed_message);
    let char_count = composed_message.chars().count();
    if sentence_count > 4 || char_count > 650 {
        return Some(
            "response is too long; keep to 1-2 concise sentences unless user asks for detail"
                .to_string(),
        );
    }
    let url_count = extract_plain_urls_from_text(composed_message).len();
    if url_count > 1 && !user_requested_sources(trusted_user_message) {
        return Some(
            "response includes unsolicited source dump; keep at most one URL unless user asks for sources"
                .to_string(),
        );
    }
    None
}

pub(crate) fn obvious_meta_compose_pattern(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    let starts_with_meta = normalized.starts_with("the assistant ")
        || normalized.starts_with("assistant is ")
        || normalized.starts_with("user asks")
        || normalized.starts_with("the user asks")
        || normalized.starts_with("quality gate")
        || normalized.starts_with("quality gate outcome")
        || normalized.starts_with("grounding gate")
        || normalized.starts_with("evidence summary")
        || normalized.starts_with("the evidence summary")
        || normalized.starts_with("draft reply");
    let contains_meta = normalized.contains("the user has")
        || normalized.contains("user has asked")
        || normalized.contains("diagnostic notes")
        || normalized.contains("draft reply says")
        || normalized.contains("the assistant is ready to help")
        || normalized.contains("quality gate")
        || normalized.contains("grounding gate")
        || normalized.contains("evidence summary")
        || normalized.contains("no relevant evidence was found")
        || normalized.contains("unsupported claim")
        || normalized.contains("ungrounded");
    starts_with_meta || contains_meta
}

pub(crate) fn compact_single_line(input: &str, max_len: usize) -> String {
    let compact = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_len {
        return compact;
    }
    let mut out = String::new();
    for ch in compact.chars().take(max_len.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

pub(crate) fn denied_outcomes_only_message(response_input: &ResponseTurnInput) -> Option<String> {
    if response_input.tool_outcomes.is_empty() {
        return None;
    }

    let all_denied = response_input
        .tool_outcomes
        .iter()
        .all(|outcome| outcome.failure_reason.is_some());
    if !all_denied {
        return None;
    }

    let mut seen = BTreeSet::new();
    let mut details = Vec::new();
    for outcome in &response_input.tool_outcomes {
        let reason = match outcome.failure_reason.as_deref() {
            Some(value) => compact_single_line(value, 120),
            None => continue,
        };
        let command = compact_single_line(
            outcome
                .attempted_command
                .as_deref()
                .unwrap_or(&outcome.tool_name),
            140,
        );
        if seen.insert((command.clone(), reason.clone())) {
            details.push((command, reason));
        }
    }

    if details.is_empty() {
        return None;
    }

    let mut message = details
        .iter()
        .take(2)
        .map(|(command, reason)| format!("I tried `{command}`, but it was blocked: {reason}."))
        .collect::<Vec<_>>()
        .join(" ");
    if details.len() > 2 {
        message.push_str(" I hit the same restriction on additional attempts.");
    }
    message.push_str(" I can try a different command path if you want.");
    Some(message)
}

pub(crate) fn strip_unexpanded_render_tokens(message: &str) -> String {
    let mut remaining = message;
    let mut out = String::new();
    loop {
        let Some(start) = remaining.find("[[") else {
            out.push_str(remaining);
            break;
        };
        out.push_str(&remaining[..start]);
        let tail = &remaining[start..];
        if tail.starts_with("[[ref:") || tail.starts_with("[[summary:") {
            if let Some(end) = tail.find("]]") {
                remaining = &tail[end + 2..];
                continue;
            }
        }
        out.push_str("[[");
        remaining = &tail[2..];
    }
    out.trim().to_string()
}
