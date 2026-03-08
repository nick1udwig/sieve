use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(name = "sieve-app")]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
enum Command {
    /// Run Sieve in long-running mode, or one-shot with `--prompt`.
    Run {
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Manage auth state.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliCommand {
    Agent,
    Run { prompt: String },
    Auth(AuthCommand),
}

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub(crate) enum AuthCommand {
    /// Print the resolved auth file path.
    Path,
    /// Complete the interactive OpenAI Codex login flow.
    Login { provider: AuthProvider },
    /// Persist explicit OpenAI Codex credentials.
    Set {
        provider: AuthProvider,
        #[arg(long)]
        access_token: String,
        #[arg(long)]
        account_id: String,
        #[arg(long)]
        refresh_token: Option<String>,
        #[arg(long)]
        expires_at_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum AuthProvider {
    #[value(name = "openai-codex")]
    OpenAiCodex,
}

impl Cli {
    pub(crate) fn into_command(self) -> CliCommand {
        match self.command {
            Command::Run { prompt } => match prompt {
                Some(prompt) => CliCommand::Run { prompt },
                None => CliCommand::Agent,
            },
            Command::Auth { command } => CliCommand::Auth(command),
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn parse_cli_args(args: &[String]) -> Result<CliCommand, String> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("sieve-app".to_string());
    argv.extend(args.iter().cloned());
    Cli::try_parse_from(argv)
        .map(Cli::into_command)
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn parse_cli_args_requires_explicit_run_for_long_running_mode() {
        let err = parse_cli_args(&[]).expect_err("reject");
        assert!(err.contains("Usage:"));
        assert!(err.contains("run"));
    }

    #[test]
    fn parse_cli_args_run_without_prompt_enters_agent_mode() {
        assert_eq!(
            parse_cli_args(&args(&["run"])).expect("parse"),
            CliCommand::Agent
        );
    }

    #[test]
    fn parse_cli_args_requires_prompt_flag_for_one_shot_mode() {
        let err =
            parse_cli_args(&args(&["run", "review", "workspace", "status"])).expect_err("reject");
        assert!(err.contains("unexpected argument"));
        assert!(err.contains("review"));
    }

    #[test]
    fn parse_cli_args_accepts_run_prompt() {
        assert_eq!(
            parse_cli_args(&args(&["run", "--prompt", "review workspace status"])).expect("parse"),
            CliCommand::Run {
                prompt: "review workspace status".to_string(),
            }
        );
    }

    #[test]
    fn parse_cli_args_accepts_auth_login() {
        assert_eq!(
            parse_cli_args(&args(&["auth", "login", "openai-codex"])).expect("parse"),
            CliCommand::Auth(AuthCommand::Login {
                provider: AuthProvider::OpenAiCodex,
            })
        );
    }

    #[test]
    fn parse_cli_args_accepts_auth_set_flags() {
        assert_eq!(
            parse_cli_args(&args(&[
                "auth",
                "set",
                "openai-codex",
                "--access-token",
                "token-123",
                "--account-id",
                "acc-123",
                "--refresh-token",
                "refresh-123",
                "--expires-at-ms",
                "42",
            ]))
            .expect("parse"),
            CliCommand::Auth(AuthCommand::Set {
                provider: AuthProvider::OpenAiCodex,
                access_token: "token-123".to_string(),
                account_id: "acc-123".to_string(),
                refresh_token: Some("refresh-123".to_string()),
                expires_at_ms: Some(42),
            })
        );
    }
}
