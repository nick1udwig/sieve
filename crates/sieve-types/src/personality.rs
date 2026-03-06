use serde::{Deserialize, Serialize};

/// Emoji policy for user-facing responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmojiPolicy {
    #[default]
    Auto,
    Avoid,
    Light,
}

/// Brevity contract for user-facing responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ResponseVerbosity {
    #[default]
    Concise,
    Detailed,
    Telegraph,
}

/// Persisted or turn-scoped personality overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PersonalitySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji_policy: Option<EmojiPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<ResponseVerbosity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_instructions: Vec<String>,
}

/// Final resolved personality contract for a response turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPersonality {
    pub identity: String,
    pub style: String,
    pub emoji_policy: EmojiPolicy,
    pub verbosity: ResponseVerbosity,
    pub channel_guidance: Vec<String>,
    pub custom_instructions: Vec<String>,
}

impl Default for ResolvedPersonality {
    fn default() -> Self {
        Self {
            identity: "helpful, optimistic, cheerful personal assistant".to_string(),
            style: "clear and concise".to_string(),
            emoji_policy: EmojiPolicy::Auto,
            verbosity: ResponseVerbosity::Concise,
            channel_guidance: Vec::new(),
            custom_instructions: Vec::new(),
        }
    }
}
