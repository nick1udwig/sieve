use crate::text_match::{contains_phrase_words, contains_word};
use sieve_llm::{ResponseTurnInput, ResponseTurnOutput};
use std::collections::BTreeSet;

pub(crate) fn requires_output_visibility(input: &ResponseTurnInput) -> bool {
    !non_empty_output_ref_ids(input).is_empty()
        && user_explicitly_requests_output_visibility(&input.trusted_user_message)
}

pub(crate) fn non_empty_output_ref_ids(input: &ResponseTurnInput) -> BTreeSet<String> {
    input
        .tool_outcomes
        .iter()
        .flat_map(|outcome| outcome.refs.iter())
        .filter(|ref_metadata| {
            output_ref_requires_visibility(&ref_metadata.kind) && ref_metadata.byte_count > 0
        })
        .map(|ref_metadata| ref_metadata.ref_id.clone())
        .collect()
}

pub(crate) fn response_has_visible_selected_output(
    input: &ResponseTurnInput,
    response: &ResponseTurnOutput,
) -> bool {
    let output_ref_ids = non_empty_output_ref_ids(input);
    response.referenced_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id) && response.message.contains(&format!("[[ref:{ref_id}]]"))
    }) || response.summarized_ref_ids.iter().any(|ref_id| {
        output_ref_ids.contains(ref_id)
            && response.message.contains(&format!("[[summary:{ref_id}]]"))
    })
}

fn user_explicitly_requests_output_visibility(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    contains_word(&lower, "output")
        || contains_word(&lower, "stdout")
        || contains_word(&lower, "stderr")
        || contains_phrase_words(&lower, &["contents", "of"])
        || contains_phrase_words(&lower, &["content", "of"])
        || contains_phrase_words(&lower, &["show", "the", "result"])
        || contains_phrase_words(&lower, &["show", "me", "the", "result"])
        || contains_phrase_words(&lower, &["run", "exactly"])
        || (contains_phrase_words(&lower, &["what", "did"]) && contains_word(&lower, "return"))
}

fn output_ref_requires_visibility(kind: &str) -> bool {
    matches!(kind, "stdout" | "stderr")
}
