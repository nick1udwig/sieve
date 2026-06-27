use crate::shell_words::command_words;
use crate::text_match::{contains_phrase_words, contains_word};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BrowserActionClass {
    Navigate,
    Inspect,
    Interact,
    Extract,
    Other,
}

impl BrowserActionClass {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::Inspect => "inspect",
            Self::Interact => "interact",
            Self::Extract => "extract",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct AgentBrowserObservation {
    pub(super) action_class: BrowserActionClass,
    pub(super) session_name: Option<String>,
    pub(super) target_url: Option<String>,
}

pub(super) fn parse_agent_browser_observation(command: &str) -> Option<AgentBrowserObservation> {
    let tokens = command_words(command).collect::<Vec<_>>();
    if shell_basename(tokens.first().copied().unwrap_or_default()) != "agent-browser" {
        return None;
    }

    let session_name = find_flag_value(&tokens, "--session");
    let action_class = match (tokens.get(1).copied(), tokens.get(2).copied()) {
        (Some("open" | "goto" | "navigate"), _) => BrowserActionClass::Navigate,
        (Some("tab"), Some("new")) => BrowserActionClass::Navigate,
        (Some("record"), Some("start" | "restart")) => BrowserActionClass::Navigate,
        (Some("snapshot"), _) => BrowserActionClass::Inspect,
        (Some("get" | "is" | "screenshot" | "pdf" | "download"), _) => BrowserActionClass::Extract,
        (
            Some(
                "click" | "dblclick" | "hover" | "focus" | "check" | "uncheck" | "select" | "drag"
                | "scroll" | "scrollintoview" | "scrollinto" | "mouse" | "fill" | "type"
                | "keyboard" | "upload" | "storage",
            ),
            _,
        ) => BrowserActionClass::Interact,
        _ => BrowserActionClass::Other,
    };

    let target_url = match (tokens.get(1).copied(), tokens.get(2).copied()) {
        (Some("open" | "goto" | "navigate"), _) => tokens.get(2).map(|value| (*value).to_string()),
        (Some("tab"), Some("new")) => tokens.get(3).map(|value| (*value).to_string()),
        (Some("record"), Some("start" | "restart")) => {
            tokens.get(4).map(|value| (*value).to_string())
        }
        _ => None,
    };

    Some(AgentBrowserObservation {
        action_class,
        session_name,
        target_url,
    })
}

pub(super) fn infer_interstitial_kind(excerpt: &str) -> Option<&'static str> {
    let lower = excerpt.to_ascii_lowercase();
    if lower.contains("google.com/sorry")
        || lower.contains("sorry/index")
        || contains_phrase_words(&lower, &["unusual", "traffic"])
        || contains_word(&lower, "captcha")
    {
        return Some("anti_bot");
    }
    if contains_phrase_words(&lower, &["sign", "in"])
        || contains_phrase_words(&lower, &["log", "in"])
        || contains_word(&lower, "login")
    {
        return Some("login");
    }
    if contains_phrase_words(&lower, &["before", "you", "continue"])
        || contains_word(&lower, "consent")
    {
        return Some("consent");
    }
    if contains_phrase_words(&lower, &["age", "restricted"])
        || contains_phrase_words(&lower, &["confirm", "your", "age"])
    {
        return Some("age_gate");
    }
    if contains_phrase_words(&lower, &["subscribe", "to", "continue"])
        || contains_phrase_words(&lower, &["membership", "required"])
        || contains_word(&lower, "paywall")
    {
        return Some("paywall");
    }
    None
}

pub(super) fn infer_browser_page_state(
    observation: &AgentBrowserObservation,
    excerpt: Option<&str>,
) -> &'static str {
    let Some(excerpt) = excerpt else {
        return "empty";
    };
    let has_result_items = excerpt_has_result_item_markers(excerpt);
    if has_result_items {
        return if browser_target_looks_like_search(observation.target_url.as_deref()) {
            "result_list"
        } else if matches!(
            observation.action_class,
            BrowserActionClass::Inspect | BrowserActionClass::Extract
        ) {
            "answer_item"
        } else {
            "detail_page"
        };
    }
    if let Some(kind) = infer_interstitial_kind(excerpt) {
        return if kind == "anti_bot" {
            "block_page"
        } else {
            "interstitial"
        };
    }

    let nonempty_lines = excerpt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if nonempty_lines.is_empty() {
        return "empty";
    }
    if nonempty_lines.iter().all(|line| is_plain_url_line(line)) {
        return "url_only";
    }
    if nonempty_lines.len() <= 2 && nonempty_lines.iter().any(|line| is_plain_url_line(line)) {
        return "title_only";
    }
    if browser_target_looks_like_search(observation.target_url.as_deref()) {
        return "result_list";
    }
    if matches!(
        observation.action_class,
        BrowserActionClass::Inspect | BrowserActionClass::Extract
    ) {
        return "answer_item";
    }
    "detail_page"
}

fn shell_basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn find_flag_value(tokens: &[&str], flag: &str) -> Option<String> {
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i];
        if token == flag {
            return tokens.get(i + 1).map(|value| (*value).to_string());
        }
        if let Some(value) = token.strip_prefix(&format!("{flag}=")) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
        i += 1;
    }
    None
}

fn is_plain_url_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("https://") || trimmed.starts_with("http://")
}

fn excerpt_has_result_item_markers(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    lower.contains("/watch?v=")
        || lower.contains("/shorts/")
        || lower.contains("- heading \"")
        || lower.contains("[level=3]")
        || (lower.contains("- main:") && lower.contains("- /url: /@"))
}

fn browser_target_looks_like_search(target_url: Option<&str>) -> bool {
    let Some(target_url) = target_url else {
        return false;
    };
    let lower = target_url.to_ascii_lowercase();
    lower.contains("/results?")
        || lower.contains("search_query=")
        || lower.contains("tbm=vid")
        || lower.contains("/search?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_browser_observation_skips_leading_env_assignments() {
        let observation = parse_agent_browser_observation(
            "AGENT_BROWSER_TIMEOUT=30 agent-browser open https://example.com --session docs",
        )
        .expect("env-prefixed agent-browser command should be parsed");

        assert_eq!(observation.action_class, BrowserActionClass::Navigate);
        assert_eq!(
            observation.target_url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(observation.session_name.as_deref(), Some("docs"));
    }

    #[test]
    fn infer_interstitial_kind_matches_real_markers() {
        assert_eq!(
            infer_interstitial_kind("Before you continue, confirm consent settings."),
            Some("consent")
        );
        assert_eq!(
            infer_interstitial_kind("Subscribe to continue reading this article."),
            Some("paywall")
        );
    }

    #[test]
    fn infer_interstitial_kind_rejects_marker_substrings() {
        assert_eq!(
            infer_interstitial_kind(
                "Article text mentions preconsent flows, recaptchaToken metadata, and paywalling economics.",
            ),
            None
        );
    }
}
