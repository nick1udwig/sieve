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

fn first_shell_word(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

pub(crate) fn classify_bash_action(command: &str) -> BashActionClass {
    let cmd = first_shell_word(command)
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
    if lower.contains("missing capability") {
        "missing_capability"
    } else if lower.contains("uncertain command") || lower.contains("unsupported") {
        "uncertain_shape"
    } else if lower.contains("denied by mode")
        || lower.contains("blocked")
        || lower.contains("not allowed")
    {
        "blocked_mode"
    } else {
        "runtime_failure"
    }
}
