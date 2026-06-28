pub(crate) fn command_words_for_simple_exec(command: &str) -> Vec<&str> {
    let words = command.split_whitespace().collect::<Vec<_>>();
    strip_words_for_simple_exec(&words)
}

pub(crate) fn command_words_for_simple_argv(argv: &[String]) -> Vec<&str> {
    let words = argv.iter().map(String::as_str).collect::<Vec<_>>();
    strip_words_for_simple_exec(&words)
}

fn strip_words_for_simple_exec<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let mut start = 0usize;
    loop {
        let before = start;
        while words
            .get(start)
            .is_some_and(|token| is_shell_assignment(token))
        {
            start += 1;
        }
        if shell_basename(words.get(start).copied().unwrap_or_default()) == "sudo"
            && words
                .get(start + 1)
                .is_some_and(|arg| !arg.starts_with('-'))
        {
            start += 1;
            continue;
        }
        if shell_basename(words.get(start).copied().unwrap_or_default()) == "env" {
            let mut next = start + 1;
            while words.get(next).is_some_and(|arg| is_env_assignment(arg)) {
                next += 1;
            }
            if next > start + 1 && next < words.len() {
                start = next;
                continue;
            }
        }
        if start == before {
            return words[start..].to_vec();
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

fn is_env_assignment(token: &str) -> bool {
    token
        .split_once('=')
        .is_some_and(|(name, _)| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_words_for_simple_exec_skips_env_and_plain_sudo() {
        let words = command_words_for_simple_exec("A=1 sudo B=2 curl -sS https://example.com");

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }

    #[test]
    fn command_words_for_simple_exec_skips_env_command_wrapper() {
        let words = command_words_for_simple_exec("sudo env A=1 curl -sS https://example.com");

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }

    #[test]
    fn command_words_for_simple_argv_skips_command_wrappers() {
        let argv = vec![
            "sudo".to_string(),
            "env".to_string(),
            "A=1".to_string(),
            "curl".to_string(),
            "-sS".to_string(),
            "https://example.com".to_string(),
        ];
        let words = command_words_for_simple_argv(&argv);

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }
}
