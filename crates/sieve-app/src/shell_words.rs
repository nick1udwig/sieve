pub(crate) fn command_words(command: &str) -> impl Iterator<Item = &str> + '_ {
    command
        .split_whitespace()
        .skip_while(|token| is_shell_assignment(token))
}

pub(crate) fn command_words_for_simple_exec(command: &str) -> Vec<&str> {
    let mut words = command.split_whitespace().collect::<Vec<_>>();
    loop {
        let before = words.len();
        words = words
            .into_iter()
            .skip_while(|token| is_shell_assignment(token))
            .collect();
        if shell_basename(words.first().copied().unwrap_or_default()) == "sudo"
            && words.get(1).is_some_and(|arg| !arg.starts_with('-'))
        {
            words.remove(0);
        }
        if words.len() == before {
            return words;
        }
    }
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

fn shell_basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
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

    #[test]
    fn command_words_for_simple_exec_skips_env_and_plain_sudo() {
        let words = command_words_for_simple_exec("A=1 sudo B=2 curl -sS https://example.com");

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }
}
