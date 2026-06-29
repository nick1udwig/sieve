pub(crate) fn truncate_for_error(input: &str, max_chars: usize) -> String {
    let Some((cut_at, _)) = input.char_indices().nth(max_chars) else {
        return input.to_string();
    };
    format!("{}...[truncated]", &input[..cut_at])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_error_uses_char_boundaries() {
        let input = format!("{}é{}", "a".repeat(3), "tail");

        assert_eq!(truncate_for_error(&input, 4), "aaaé...[truncated]");
    }
}
