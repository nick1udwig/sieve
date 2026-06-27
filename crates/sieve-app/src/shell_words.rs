pub(crate) fn command_words(command: &str) -> impl Iterator<Item = &str> + '_ {
    command
        .split_whitespace()
        .skip_while(|token| is_shell_assignment(token))
}

pub(crate) fn first_command_word(command: &str) -> Option<&str> {
    command_words(command).next()
}

fn is_shell_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_words_skips_leading_env_assignments_only() {
        let words = command_words("A=1 _B=two curl X=literal").collect::<Vec<_>>();

        assert_eq!(words, vec!["curl", "X=literal"]);
    }

    #[test]
    fn command_words_keeps_invalid_assignments_as_command_words() {
        let words = command_words("1A=bad curl").collect::<Vec<_>>();

        assert_eq!(words, vec!["1A=bad", "curl"]);
    }
}
