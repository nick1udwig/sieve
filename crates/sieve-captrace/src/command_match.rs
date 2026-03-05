use std::path::Path;

pub(crate) fn argv_matches_command(argv: &[String], command: &str) -> bool {
    let Some(first) = argv.first() else {
        return false;
    };

    if token_matches_command(first, command) {
        return true;
    }

    if first == "sudo" {
        if let Some(second) = argv.get(1) {
            return token_matches_command(second, command);
        }
    }

    false
}

pub(crate) fn args_after_command<'a>(argv: &'a [String], command: &str) -> &'a [String] {
    let Some(first) = argv.first() else {
        return &[];
    };

    if token_matches_command(first, command) {
        return argv.get(1..).unwrap_or(&[]);
    }
    if first == "sudo" {
        if let Some(second) = argv.get(1) {
            if token_matches_command(second, command) {
                return argv.get(2..).unwrap_or(&[]);
            }
        }
    }
    &[]
}

pub(crate) fn token_matches_command(token: &str, command: &str) -> bool {
    if token == command || token.ends_with(&format!("/{command}")) {
        return true;
    }

    let Some(command_basename) = Path::new(command).file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    token == command_basename || token.ends_with(&format!("/{command_basename}"))
}
