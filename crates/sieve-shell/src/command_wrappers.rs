#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandWrapperView<'a, T> {
    pub argv: &'a [T],
    pub offset: usize,
}

pub fn strip_command_wrappers<T: AsRef<str>>(mut argv: &[T]) -> CommandWrapperView<'_, T> {
    let mut offset = 0usize;
    loop {
        let sudo = strip_sudo_wrapper(argv);
        if sudo.offset > 0 {
            offset += sudo.offset;
            argv = sudo.argv;
            continue;
        }
        let env = strip_env_wrapper(argv);
        if env.offset > 0 {
            offset += env.offset;
            argv = env.argv;
            continue;
        }
        return CommandWrapperView { argv, offset };
    }
}

fn strip_sudo_wrapper<T: AsRef<str>>(argv: &[T]) -> CommandWrapperView<'_, T> {
    if basename(argv.first()) == Some("sudo") {
        if let Some(next) = command_after_wrapper_options(argv, 0, true, sudo_flag_value_words) {
            return CommandWrapperView {
                argv: &argv[next..],
                offset: next,
            };
        }
    }
    CommandWrapperView { argv, offset: 0 }
}

fn strip_env_wrapper<T: AsRef<str>>(argv: &[T]) -> CommandWrapperView<'_, T> {
    if basename(argv.first()) == Some("env") {
        if let Some(next) = command_after_wrapper_options(argv, 0, true, env_flag_value_words) {
            return CommandWrapperView {
                argv: &argv[next..],
                offset: next,
            };
        }
    }
    CommandWrapperView { argv, offset: 0 }
}

fn command_after_wrapper_options<T: AsRef<str>>(
    argv: &[T],
    start: usize,
    skip_assignments: bool,
    flag_value_words: fn(&str) -> Option<usize>,
) -> Option<usize> {
    let mut next = start + 1;
    while let Some(arg) = argv.get(next).map(AsRef::as_ref) {
        if skip_assignments && is_env_assignment(arg) {
            next += 1;
            continue;
        }
        let Some(value_words) = flag_value_words(arg) else {
            break;
        };
        if next + value_words >= argv.len() {
            return None;
        }
        next += 1 + value_words;
    }
    argv.get(next)
        .is_some_and(|arg| !arg.as_ref().starts_with('-'))
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

fn is_env_assignment(arg: &str) -> bool {
    arg.split_once('=')
        .is_some_and(|(name, _)| !name.is_empty())
}

fn basename<T: AsRef<str>>(value: Option<&T>) -> Option<&str> {
    value?.as_ref().rsplit('/').next()
}
