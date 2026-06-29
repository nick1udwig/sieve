use sieve_shell::strip_command_wrappers;

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
        let inner = strip_command_wrappers(&words[start..]);
        if inner.offset > 0 {
            start += inner.offset;
            continue;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_words_for_simple_exec_skips_env_and_plain_sudo() {
        let words = command_words_for_simple_exec("A=1 sudo B=2 curl -sS https://example.com");

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }

    #[test]
    fn command_words_for_simple_exec_skips_no_value_sudo_flags() {
        let words = command_words_for_simple_exec("sudo -n -E curl -sS https://example.com");

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }

    #[test]
    fn command_words_for_simple_exec_skips_sudo_flags_with_values() {
        let words = command_words_for_simple_exec(
            "sudo -u root --preserve-env=PATH curl -sS https://example.com",
        );

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }

    #[test]
    fn command_words_for_simple_exec_skips_env_flags() {
        let words =
            command_words_for_simple_exec("env -i -u TOKEN A=1 curl -sS https://example.com");

        assert_eq!(words, vec!["curl", "-sS", "https://example.com"]);
    }

    #[test]
    fn command_words_for_simple_exec_skips_option_separators() {
        let words = command_words_for_simple_exec(
            "sudo -- env --ignore-environment A=1 curl -sS https://example.com",
        );

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
