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
        if shell_basename(words.get(start).copied().unwrap_or_default()) == "sudo" {
            if let Some(next) = command_after_sudo_wrapper(words, start) {
                start = next;
                continue;
            }
        }
        if shell_basename(words.get(start).copied().unwrap_or_default()) == "env" {
            if let Some(next) = command_after_env_wrapper(words, start) {
                start = next;
                continue;
            }
        }
        if start == before {
            return words[start..].to_vec();
        }
    }
}

fn command_after_sudo_wrapper(words: &[&str], start: usize) -> Option<usize> {
    command_after_wrapper_options(words, start, false, sudo_flag_value_words)
}

fn command_after_env_wrapper(words: &[&str], start: usize) -> Option<usize> {
    command_after_wrapper_options(words, start, true, env_flag_value_words)
}

fn command_after_wrapper_options(
    words: &[&str],
    start: usize,
    skip_assignments: bool,
    flag_value_words: fn(&str) -> Option<usize>,
) -> Option<usize> {
    let mut next = start + 1;
    while let Some(arg) = words.get(next) {
        if skip_assignments && is_env_assignment(arg) {
            next += 1;
            continue;
        }
        let Some(value_words) = flag_value_words(arg) else {
            break;
        };
        if next + value_words >= words.len() {
            return None;
        }
        next += 1 + value_words;
    }
    words
        .get(next)
        .is_some_and(|arg| !arg.starts_with('-'))
        .then_some(next)
}

fn sudo_flag_value_words(token: &str) -> Option<usize> {
    const NO_VALUE_LONG: &[&str] = &[
        "askpass",
        "background",
        "non-interactive",
        "preserve-env",
        "reset-timestamp",
        "set-home",
        "stdin",
        "validate",
    ];
    const VALUE_LONG: &[&str] = &[
        "chdir",
        "close-from",
        "command-timeout",
        "group",
        "host",
        "login-class",
        "prompt",
        "role",
        "type",
        "user",
    ];
    if let Some(flag) = token.strip_prefix("--") {
        return long_flag_value_words(flag, NO_VALUE_LONG, VALUE_LONG);
    }
    short_flag_value_words(token, "AbEHknSv", "CDghprtTu")
}

fn env_flag_value_words(token: &str) -> Option<usize> {
    const NO_VALUE_LONG: &[&str] = &["debug", "ignore-environment", "null"];
    const VALUE_LONG: &[&str] = &["argv0", "chdir", "split-string", "unset"];
    if let Some(flag) = token.strip_prefix("--") {
        return long_flag_value_words(flag, NO_VALUE_LONG, VALUE_LONG);
    }
    short_flag_value_words(token, "0iv", "aCSu")
}

fn long_flag_value_words(flag: &str, no_value: &[&str], value: &[&str]) -> Option<usize> {
    if flag.is_empty() {
        return Some(0);
    }
    let name = flag.split_once('=').map_or(flag, |(name, _)| name);
    if no_value.contains(&name) {
        return Some(0);
    }
    value
        .contains(&name)
        .then_some(usize::from(!flag.contains('=')))
}

fn short_flag_value_words(token: &str, no_value: &str, value: &str) -> Option<usize> {
    if token == "--" {
        return Some(0);
    }
    if !token.starts_with('-') || token == "-" {
        return None;
    }
    let mut chars = token[1..].chars().peekable();
    while let Some(flag) = chars.next() {
        if no_value.contains(flag) {
            continue;
        }
        if value.contains(flag) {
            return Some(usize::from(chars.peek().is_none()));
        }
        return None;
    }
    Some(0)
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
