pub(crate) fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

pub(crate) fn word_matches_at(text: &str, idx: usize, word: &str) -> bool {
    let before = text[..idx]
        .chars()
        .next_back()
        .is_none_or(|ch| !is_word_char(ch));
    let after = text[idx + word.len()..]
        .chars()
        .next()
        .is_none_or(|ch| !is_word_char(ch));
    before && after
}

pub(crate) fn starts_with_word(text: &str, word: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with(word) && word_matches_at(trimmed, 0, word)
}

pub(crate) fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word)
        .any(|(idx, _)| word_matches_at(text, idx, word))
}

pub(crate) fn contains_any_word(text: &str, words: &[&str]) -> bool {
    words.iter().any(|word| contains_word(text, word))
}

pub(crate) fn contains_phrase_words(text: &str, phrase: &[&str]) -> bool {
    let tokens = text
        .split(|ch: char| !is_word_char(ch))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    tokens.windows(phrase.len()).any(|window| window == phrase)
}

pub(crate) fn contains_token_sequence(text_lower: &str, candidate: &str) -> bool {
    let candidate_lower = candidate.to_ascii_lowercase();
    let phrase = candidate_lower
        .split(|ch: char| !is_word_char(ch))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    !phrase.is_empty() && contains_phrase_words(text_lower, &phrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_token_sequence_matches_punctuation_separated_tokens() {
        assert!(contains_token_sequence(
            "how is you are starting going",
            "you-are-starting"
        ));
        assert!(contains_token_sequence("build modex status", "build modex"));
    }

    #[test]
    fn contains_token_sequence_rejects_substrings() {
        assert!(!contains_token_sequence("how is model work going", "mod"));
        assert!(!contains_token_sequence("resume unrelated", ""));
    }
}
