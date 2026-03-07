use crate::ingress::PromptSource;
use sieve_types::{
    DeliveryChannel, DeliveryContext, EmojiPolicy, InteractionModality, PersonalitySettings,
    ResolvedPersonality, ResponseVerbosity,
};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const PERSONALITY_SCHEMA_VERSION: u16 = 1;
const DEFAULT_IDENTITY: &str = "helpful, optimistic, cheerful personal assistant";
const DEFAULT_STYLE: &str = "clear and concise";
static PERSONALITY_TMP_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PersonalityStateFile {
    schema_version: u16,
    #[serde(default)]
    settings: PersonalitySettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PersonalityDirective {
    settings: PersonalitySettings,
    persist: bool,
    style_only: bool,
    reset: bool,
    acknowledgement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonalityTurnResolution {
    pub(crate) delivery_context: DeliveryContext,
    pub(crate) resolved_personality: ResolvedPersonality,
    pub(crate) acknowledgement: Option<String>,
}

pub(crate) fn personality_state_path(sieve_home: &Path) -> PathBuf {
    sieve_home.join("state/personality.json")
}

pub(crate) fn resolve_turn_personality(
    sieve_home: &Path,
    source: PromptSource,
    destination: Option<&str>,
    input_modality: InteractionModality,
    response_modality: InteractionModality,
    trusted_user_message: &str,
) -> Result<PersonalityTurnResolution, String> {
    let delivery_context = DeliveryContext {
        channel: delivery_channel_for_source(source),
        destination: destination.map(str::to_string),
        input_modality,
        response_modality,
    };

    let path = personality_state_path(sieve_home);
    let mut persisted = load_personality_settings(&path)?;
    let directive = parse_personality_directive(trusted_user_message);
    let turn_settings = if let Some(directive) = directive.as_ref() {
        if directive.reset {
            persisted = PersonalitySettings::default();
        } else if directive.persist {
            persisted = merge_settings(&persisted, &directive.settings);
        }
        if directive.persist || directive.reset {
            save_personality_settings(&path, &persisted)?;
        }
        directive.settings.clone()
    } else {
        PersonalitySettings::default()
    };
    let effective_turn_settings = if directive
        .as_ref()
        .map(|value| value.persist)
        .unwrap_or(false)
    {
        PersonalitySettings::default()
    } else {
        turn_settings.clone()
    };

    Ok(PersonalityTurnResolution {
        delivery_context: delivery_context.clone(),
        resolved_personality: resolve_personality(
            &delivery_context,
            &persisted,
            &effective_turn_settings,
        ),
        acknowledgement: directive
            .as_ref()
            .filter(|value| value.style_only)
            .map(|value| value.acknowledgement.clone()),
    })
}

fn delivery_channel_for_source(source: PromptSource) -> DeliveryChannel {
    match source {
        PromptSource::Stdin => DeliveryChannel::Stdin,
        PromptSource::Telegram => DeliveryChannel::Telegram,
    }
}

fn load_personality_settings(path: &Path) -> Result<PersonalitySettings, String> {
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(PersonalitySettings::default())
        }
        Err(err) => return Err(format!("failed reading {}: {err}", path.display())),
    };
    let parsed: PersonalityStateFile = serde_json::from_str(&body)
        .map_err(|err| format!("failed parsing {}: {err}", path.display()))?;
    if parsed.schema_version != PERSONALITY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported personality schema_version {} in {}",
            parsed.schema_version,
            path.display()
        ));
    }
    Ok(parsed.settings)
}

fn save_personality_settings(path: &Path, settings: &PersonalitySettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating {}: {err}", parent.display()))?;
    }
    let payload = PersonalityStateFile {
        schema_version: PERSONALITY_SCHEMA_VERSION,
        settings: settings.clone(),
    };
    let encoded = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed encoding personality state: {err}"))?;
    let nonce = PERSONALITY_TMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_extension(format!("json.tmp.{}.{}", std::process::id(), nonce));
    fs::write(&tmp_path, encoded)
        .map_err(|err| format!("failed writing {}: {err}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).map_err(|err| {
        format!(
            "failed renaming {} to {}: {err}",
            tmp_path.display(),
            path.display()
        )
    })
}

fn resolve_personality(
    delivery_context: &DeliveryContext,
    persisted: &PersonalitySettings,
    turn: &PersonalitySettings,
) -> ResolvedPersonality {
    let mut resolved = ResolvedPersonality {
        identity: DEFAULT_IDENTITY.to_string(),
        style: DEFAULT_STYLE.to_string(),
        emoji_policy: turn
            .emoji_policy
            .or(persisted.emoji_policy)
            .unwrap_or_else(|| default_emoji_policy(delivery_context)),
        verbosity: turn
            .verbosity
            .or(persisted.verbosity)
            .unwrap_or_else(|| default_verbosity(delivery_context)),
        channel_guidance: channel_guidance(delivery_context),
        custom_instructions: dedupe_preserve_order(
            persisted
                .custom_instructions
                .iter()
                .chain(turn.custom_instructions.iter())
                .cloned()
                .collect(),
        ),
    };
    if delivery_context.response_modality == InteractionModality::Audio {
        resolved.emoji_policy = EmojiPolicy::Avoid;
    }
    resolved
}

fn default_emoji_policy(delivery_context: &DeliveryContext) -> EmojiPolicy {
    if delivery_context.response_modality == InteractionModality::Audio {
        return EmojiPolicy::Avoid;
    }
    match delivery_context.channel {
        DeliveryChannel::Telegram => EmojiPolicy::Light,
        DeliveryChannel::Stdin => EmojiPolicy::Avoid,
    }
}

fn default_verbosity(delivery_context: &DeliveryContext) -> ResponseVerbosity {
    match delivery_context.channel {
        DeliveryChannel::Telegram => ResponseVerbosity::Concise,
        DeliveryChannel::Stdin => ResponseVerbosity::Concise,
    }
}

fn channel_guidance(delivery_context: &DeliveryContext) -> Vec<String> {
    let mut guidance = match delivery_context.channel {
        DeliveryChannel::Telegram => vec![
            "Write in short chat-sized paragraphs suited to Telegram.".to_string(),
            "Keep the tone warm and direct, not essay-like.".to_string(),
        ],
        DeliveryChannel::Stdin => vec![
            "Use plain text that reads well in a terminal.".to_string(),
            "Prefer direct, compact wording over chatty filler.".to_string(),
        ],
    };
    match delivery_context.response_modality {
        InteractionModality::Audio => guidance.push(
            "Write as spoken audio: natural cadence, contractions, and no markdown-dependent phrasing."
                .to_string(),
        ),
        InteractionModality::Image => guidance.push(
            "If discussing an image result, keep wording concise and descriptive.".to_string(),
        ),
        InteractionModality::Text => {}
    }
    guidance
}

fn merge_settings(
    base: &PersonalitySettings,
    overlay: &PersonalitySettings,
) -> PersonalitySettings {
    PersonalitySettings {
        emoji_policy: overlay.emoji_policy.or(base.emoji_policy),
        verbosity: overlay.verbosity.or(base.verbosity),
        custom_instructions: if overlay.custom_instructions.is_empty() {
            base.custom_instructions.clone()
        } else {
            dedupe_preserve_order(overlay.custom_instructions.clone())
        },
    }
}

fn parse_personality_directive(trusted_user_message: &str) -> Option<PersonalityDirective> {
    let normalized = trusted_user_message.trim();
    if normalized.is_empty() {
        return None;
    }
    let lower = normalized.to_ascii_lowercase();
    if !contains_style_request(&lower) {
        return None;
    }

    let reset = contains_any(
        &lower,
        &[
            "reset personality",
            "reset your personality",
            "use the default style",
            "use the default personality",
            "back to normal",
            "normal style",
            "default style again",
        ],
    );
    let style_only = is_style_only_request(&lower);
    let persist = if contains_any(
        &lower,
        &[
            "for this reply",
            "for this response",
            "for this message",
            "this time",
            "for now",
            "right now",
        ],
    ) {
        false
    } else if contains_any(
        &lower,
        &[
            "from now on",
            "going forward",
            "by default",
            "default to",
            "every reply",
            "every response",
            "in general",
            "always",
        ],
    ) {
        true
    } else {
        style_only
    };

    let mut settings = PersonalitySettings::default();
    if contains_any(
        &lower,
        &[
            "a lot of emoji",
            "a lot of emojis",
            "lots of emoji",
            "lots of emojis",
            "many emojis",
            "multiple emojis",
            "2+ emoji",
            "2+ emojis",
            "emoji heavy",
            "emoji-heavy",
        ],
    ) {
        settings.emoji_policy = Some(EmojiPolicy::Auto);
        settings.custom_instructions.push(
            "Use an emoji-heavy chat style with multiple emojis in text replies.".to_string(),
        );
    } else if contains_any(
        &lower,
        &[
            "don't use emoji",
            "dont use emoji",
            "don't use emojis",
            "dont use emojis",
            "no emoji",
            "no emojis",
            "without emoji",
            "without emojis",
            "skip emoji",
            "skip emojis",
        ],
    ) {
        settings.emoji_policy = Some(EmojiPolicy::Avoid);
    } else if contains_any(
        &lower,
        &[
            "use emoji",
            "use emojis",
            "using emoji",
            "using emojis",
            "start using emoji",
            "start using emojis",
            "with emoji",
            "with emojis",
            "more emoji",
            "more emojis",
        ],
    ) {
        settings.emoji_policy = Some(EmojiPolicy::Light);
    }

    if contains_any(
        &lower,
        &["telegraph", "telegram style telegraph", "conserve tokens"],
    ) {
        settings.verbosity = Some(ResponseVerbosity::Telegraph);
    } else if contains_any(
        &lower,
        &[
            "more terse",
            "terser",
            "brief",
            "concise",
            "shorter",
            "keep it short",
            "short replies",
        ],
    ) {
        settings.verbosity = Some(ResponseVerbosity::Concise);
    } else if contains_any(
        &lower,
        &[
            "more detailed",
            "be detailed",
            "more detail",
            "detailed",
            "longer",
            "more verbose",
            "thorough",
        ],
    ) {
        settings.verbosity = Some(ResponseVerbosity::Detailed);
    }

    if let Some(instruction) = tone_instruction(&lower) {
        settings.custom_instructions.push(instruction.to_string());
    } else if let Some(instruction) = persona_instruction(normalized, &lower) {
        settings.custom_instructions.push(instruction);
    } else if contains_any(
        &lower,
        &[
            "pretend",
            "act like",
            "acting like",
            "behave more",
            "sound more",
            "speak more",
            "talk like",
            "write like",
            "you are my",
        ],
    ) && settings.verbosity.is_none()
    {
        settings
            .custom_instructions
            .push(compact_instruction(normalized, 220));
    }

    if !reset
        && settings.emoji_policy.is_none()
        && settings.verbosity.is_none()
        && settings.custom_instructions.is_empty()
    {
        return None;
    }

    Some(PersonalityDirective {
        acknowledgement: build_acknowledgement(reset, persist, &settings),
        settings,
        persist,
        style_only,
        reset,
    })
}

fn build_acknowledgement(reset: bool, persist: bool, settings: &PersonalitySettings) -> String {
    if reset {
        return "I'll use the default assistant style again.".to_string();
    }

    let mut parts = Vec::new();
    match settings.verbosity {
        Some(ResponseVerbosity::Concise) => parts.push("keep replies concise".to_string()),
        Some(ResponseVerbosity::Detailed) => {
            parts.push("allow more detail when it helps".to_string())
        }
        Some(ResponseVerbosity::Telegraph) => {
            parts.push("use terse, telegraph-style phrasing".to_string())
        }
        None => {}
    }
    match settings.emoji_policy {
        Some(EmojiPolicy::Avoid) => parts.push("skip emojis".to_string()),
        Some(EmojiPolicy::Light) => parts.push("use light emojis when they fit".to_string()),
        Some(EmojiPolicy::Auto) => parts.push("lean into emojis".to_string()),
        None => {}
    }
    for note in settings.custom_instructions.iter().take(2) {
        parts.push(compact_instruction(note.trim_end_matches('.'), 80).to_ascii_lowercase());
    }
    if parts.is_empty() {
        parts.push("adjust the tone".to_string());
    }

    let joined = join_with_and(&parts);
    if persist {
        format!("I'll {joined} from now on.")
    } else {
        format!("For this reply, I'll {joined}.")
    }
}

fn contains_style_request(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "emoji",
            "emojis",
            "tone",
            "personality",
            "style",
            "reply",
            "replies",
            "respond",
            "response",
            "responses",
            "speak",
            "sound",
            "behave",
            "pretend",
            "act like",
            "acting like",
            "talk like",
            "write like",
            "flirty",
            "formal",
            "casual",
            "friendly",
            "serious",
            "playful",
            "cheerful",
            "optimistic",
            "terse",
            "concise",
            "detailed",
            "telegraph",
        ],
    )
}

fn is_style_only_request(lower: &str) -> bool {
    contains_style_request(lower)
        && !contains_any(
            lower,
            &[
                "what ",
                "when ",
                "where ",
                "who ",
                "why ",
                "how ",
                "find ",
                "look up",
                "search ",
                "tell me",
                "show me",
                "summarize",
                "summary",
                "draft ",
                "write ",
                "run ",
                "execute ",
                "weather",
                "latest",
                "status",
                "explain ",
                "list ",
            ],
        )
}

fn tone_instruction(lower: &str) -> Option<&'static str> {
    if lower.contains("flirty") || lower.contains("sexy secretary") {
        Some("Use a flirtier, playful tone.")
    } else if lower.contains("formal") {
        Some("Use a more formal tone.")
    } else if lower.contains("casual") {
        Some("Use a more casual tone.")
    } else if lower.contains("friendly") {
        Some("Keep the tone friendly.")
    } else if lower.contains("playful") {
        Some("Keep the tone playful.")
    } else if lower.contains("serious") {
        Some("Keep the tone serious.")
    } else if lower.contains("cheerful") {
        Some("Keep the tone cheerful.")
    } else if lower.contains("optimistic") {
        Some("Keep the tone optimistic.")
    } else {
        None
    }
}

fn persona_instruction(normalized: &str, lower: &str) -> Option<String> {
    for marker in [
        "acting like ",
        "act like ",
        "sound like ",
        "speak like ",
        "talk like ",
        "write like ",
        "behave like ",
        "you are my ",
    ] {
        let Some(start) = lower.find(marker) else {
            continue;
        };
        let phrase = normalized[start + marker.len()..]
            .trim()
            .trim_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?'));
        if phrase.is_empty() {
            continue;
        }
        let phrase = compact_instruction(phrase, 60);
        let article_prefix = if phrase.starts_with("a ")
            || phrase.starts_with("an ")
            || phrase.starts_with("the ")
        {
            ""
        } else {
            "a "
        };
        return Some(format!("Adopt {article_prefix}{phrase} persona."));
    }
    None
}

fn dedupe_preserve_order(values: Vec<String>) -> Vec<String> {
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

fn compact_instruction(input: &str, max_len: usize) -> String {
    let compact = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_len {
        return compact;
    }
    let mut out = String::new();
    for ch in compact.chars().take(max_len.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn join_with_and(parts: &[String]) -> String {
    match parts {
        [] => "adjust the tone".to_string(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let mut out = String::new();
            for (idx, part) in parts.iter().enumerate() {
                if idx > 0 {
                    if idx + 1 == parts.len() {
                        out.push_str(", and ");
                    } else {
                        out.push_str(", ");
                    }
                }
                out.push_str(part);
            }
            out
        }
    }
}
