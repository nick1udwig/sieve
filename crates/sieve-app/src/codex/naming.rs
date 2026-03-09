use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

const SESSION_NAME_FALLBACKS: &[&str] = &[
    "avogadro",
    "curie",
    "bohr",
    "pauling",
    "mendeleev",
    "lavoisier",
    "meitner",
    "lewis",
    "dalton",
    "franklin",
];

static NEXT_FALLBACK: AtomicUsize = AtomicUsize::new(0);

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "at", "be", "by", "do", "for", "from", "how", "if", "in", "into", "it",
    "make", "need", "of", "on", "or", "please", "replace", "run", "that", "the", "this", "to",
    "use", "with",
];

pub(crate) fn session_name_from_instruction(
    instruction: &str,
    existing_names: &BTreeSet<String>,
) -> String {
    let mut parts = Vec::new();
    for token in tokenize(instruction) {
        if STOPWORDS.contains(&token.as_str()) {
            continue;
        }
        if parts.contains(&token) {
            continue;
        }
        parts.push(token);
        if parts.len() >= 3 {
            break;
        }
    }
    let base = if parts.is_empty() {
        fallback_name()
    } else {
        parts.join("-")
    };
    uniquify_name(&base, existing_names)
}

pub(crate) fn summarize_instruction(instruction: &str) -> String {
    let trimmed = instruction.trim();
    if trimmed.is_empty() {
        return "codex task".to_string();
    }
    let mut out = String::new();
    for ch in trimmed.chars() {
        if out.chars().count() >= 120 {
            break;
        }
        out.push(ch);
    }
    out
}

fn tokenize(input: &str) -> Vec<String> {
    let mut normalized = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else {
            normalized.push(' ');
        }
    }
    normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn fallback_name() -> String {
    let idx = NEXT_FALLBACK.fetch_add(1, Ordering::Relaxed);
    SESSION_NAME_FALLBACKS[idx % SESSION_NAME_FALLBACKS.len()].to_string()
}

fn uniquify_name(base: &str, existing_names: &BTreeSet<String>) -> String {
    if !existing_names.contains(base) {
        return base.to_string();
    }
    for suffix in 2.. {
        let candidate = format!("{base}-{suffix}");
        if !existing_names.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("integer suffix space exhausted")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_hyphenated_name_from_instruction() {
        let name = session_name_from_instruction(
            "Please fix the auth flow tests in this repo",
            &BTreeSet::new(),
        );
        assert_eq!(name, "fix-auth-flow");
    }

    #[test]
    fn falls_back_when_instruction_has_no_good_tokens() {
        let name = session_name_from_instruction("the and or", &BTreeSet::new());
        assert!(!name.is_empty());
        assert!(!name.contains(' '));
    }

    #[test]
    fn uniquifies_existing_names() {
        let mut existing = BTreeSet::new();
        existing.insert("fix-auth-flow".to_string());
        let name = session_name_from_instruction("fix auth flow", &existing);
        assert_eq!(name, "fix-auth-flow-2");
    }
}
