use crate::shell_words::command_words_for_simple_exec;
use crate::text_match::{contains_phrase_words, contains_word};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BashActionClass {
    Discovery,
    Fetch,
    Extract,
    Other,
}

impl BashActionClass {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Fetch => "fetch",
            Self::Extract => "extract",
            Self::Other => "other",
        }
    }
}

pub(crate) const MIN_PRIMARY_FETCH_STDOUT_BYTES: u64 = 256;

pub(crate) fn classify_bash_action(command: &str) -> BashActionClass {
    let cmd = command_words_for_simple_exec(command)
        .first()
        .copied()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match cmd.as_str() {
        "bravesearch" | "brave-search" => BashActionClass::Discovery,
        "curl" | "wget" => BashActionClass::Fetch,
        "jq" | "awk" | "sed" | "grep" | "rg" => BashActionClass::Extract,
        _ => BashActionClass::Other,
    }
}

pub(crate) fn command_targets_markdown_view(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("https://markdown.new/") || lower.contains("http://markdown.new/")
}

const ASSET_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".css", ".js",
];

fn contains_asset_extension(lower: &str) -> bool {
    ASSET_EXTENSIONS.iter().any(|extension| {
        lower.match_indices(extension).any(|(idx, _)| {
            lower[idx + extension.len()..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_ascii_alphanumeric())
        })
    })
}

pub(super) fn command_targets_likely_asset(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("imgs.search.brave.com")
        || lower.contains("favicon")
        || contains_asset_extension(&lower)
}

pub(crate) fn url_is_likely_asset(url: &str) -> bool {
    command_targets_likely_asset(url)
}

pub(super) fn command_failure_kind_from_reason(reason: &str) -> &'static str {
    let lower = reason.to_ascii_lowercase();
    if contains_phrase_words(&lower, &["missing", "capability"]) {
        "missing_capability"
    } else if contains_phrase_words(&lower, &["uncertain", "command"])
        || contains_word(&lower, "unsupported")
    {
        "uncertain_shape"
    } else if contains_phrase_words(&lower, &["denied", "by", "mode"])
        || contains_word(&lower, "blocked")
        || contains_phrase_words(&lower, &["not", "allowed"])
    {
        "blocked_mode"
    } else {
        "runtime_failure"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_failure_kind_uses_word_boundaries_for_blocked() {
        assert_eq!(
            command_failure_kind_from_reason("command was unblocked after retry"),
            "runtime_failure"
        );
    }

    #[test]
    fn classify_bash_action_skips_leading_env_assignments() {
        assert_eq!(
            classify_bash_action("HTTPS_PROXY=http://proxy curl -sS https://example.com"),
            BashActionClass::Fetch
        );
        assert_eq!(
            classify_bash_action("LC_ALL=C rg needle file.txt"),
            BashActionClass::Extract
        );
        assert_eq!(
            classify_bash_action("_TOKEN=redacted bravesearch search --query status"),
            BashActionClass::Discovery
        );
    }

    #[test]
    fn classify_bash_action_skips_plain_sudo_wrapper() {
        assert_eq!(
            classify_bash_action("sudo curl -sS https://example.com"),
            BashActionClass::Fetch
        );
    }

    #[test]
    fn classify_bash_action_skips_env_command_wrapper() {
        assert_eq!(
            classify_bash_action("sudo env TOKEN=redacted curl -sS https://example.com"),
            BashActionClass::Fetch
        );
    }
}
