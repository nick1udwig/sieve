use reqwest::Client;
use sieve_llm::{
    create_openai_codex_authorization_flow, exchange_openai_codex_authorization_code,
    parse_openai_codex_authorization_input, resolve_openai_codex_auth_json_path_from_env,
    write_openai_codex_auth_file, OpenAiCodexStoredAuth,
};
use std::io::{self, Write};

const OPENAI_CODEX_PROVIDER: &str = "openai-codex";

pub(crate) async fn maybe_run_auth_command(args: &[String]) -> Result<bool, String> {
    if args.first().map(String::as_str) != Some("auth") {
        return Ok(false);
    }

    match args.get(1).map(String::as_str) {
        Some("path") => {
            if args.len() != 2 {
                return Err(auth_usage());
            }
            println!(
                "{}",
                resolve_openai_codex_auth_json_path_from_env().display()
            );
            Ok(true)
        }
        Some("login") => {
            run_login_command(&args[2..]).await?;
            Ok(true)
        }
        Some("set") => {
            run_set_command(&args[2..])?;
            Ok(true)
        }
        _ => Err(auth_usage()),
    }
}

async fn run_login_command(args: &[String]) -> Result<(), String> {
    let provider = args.first().map(String::as_str).ok_or_else(auth_usage)?;
    if provider != OPENAI_CODEX_PROVIDER || args.len() != 1 {
        return Err(auth_usage());
    }

    let auth_path = resolve_openai_codex_auth_json_path_from_env();
    let flow = create_openai_codex_authorization_flow("sieve").map_err(|err| err.to_string())?;

    println!("Open this URL in a browser:");
    println!("{}", flow.authorization_url);
    println!();
    println!("Complete login, then paste the full redirect URL or just the authorization code.");
    println!("If the browser lands on a localhost error page, copy the URL from the address bar.");
    print!("auth> ");
    io::stdout()
        .flush()
        .map_err(|err| format!("failed flushing stdout: {err}"))?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|err| format!("failed reading authorization input: {err}"))?;
    let parsed = parse_openai_codex_authorization_input(&input).map_err(|err| err.to_string())?;
    if let Some(state) = parsed.state.as_deref() {
        if state != flow.state {
            return Err("authorization state mismatch".to_string());
        }
    }

    let http = Client::builder()
        .build()
        .map_err(|err| format!("failed to build HTTP client: {err}"))?;
    let auth = exchange_openai_codex_authorization_code(&http, &parsed.code, &flow.verifier)
        .await
        .map_err(|err| err.to_string())?;
    persist_openai_codex_auth(&auth_path, &auth)?;
    println!("saved openai-codex auth to {}", auth_path.display());
    Ok(())
}

fn run_set_command(args: &[String]) -> Result<(), String> {
    let provider = args.first().map(String::as_str).ok_or_else(auth_usage)?;
    if provider != OPENAI_CODEX_PROVIDER {
        return Err(auth_usage());
    }
    let parsed = parse_set_openai_codex_args(&args[1..])?;
    let auth_path = resolve_openai_codex_auth_json_path_from_env();
    persist_openai_codex_auth(&auth_path, &parsed)?;
    println!("saved openai-codex auth to {}", auth_path.display());
    Ok(())
}

fn persist_openai_codex_auth(
    path: &std::path::Path,
    auth: &OpenAiCodexStoredAuth,
) -> Result<(), String> {
    write_openai_codex_auth_file(path, auth)
}

fn parse_set_openai_codex_args(args: &[String]) -> Result<OpenAiCodexStoredAuth, String> {
    let mut access_token = None;
    let mut account_id = None;
    let mut refresh_token = None;
    let mut expires_at_ms = None;
    let mut idx = 0usize;

    while idx < args.len() {
        let flag = args[idx].as_str();
        let value = args
            .get(idx + 1)
            .ok_or_else(|| format!("missing value for `{flag}`"))?;
        match flag {
            "--access-token" => access_token = Some(value.clone()),
            "--account-id" => account_id = Some(value.clone()),
            "--refresh-token" => refresh_token = Some(value.clone()),
            "--expires-at-ms" => {
                expires_at_ms = Some(
                    value
                        .parse::<u64>()
                        .map_err(|err| format!("invalid `--expires-at-ms`: {err}"))?,
                )
            }
            _ => return Err(format!("unsupported flag `{flag}`")),
        }
        idx += 2;
    }

    let access_token = access_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing required `--access-token`".to_string())?;
    let account_id = account_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing required `--account-id`".to_string())?;
    let refresh_token = refresh_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Ok(OpenAiCodexStoredAuth {
        access_token,
        account_id,
        refresh_token,
        expires_at_ms,
    })
}

fn auth_usage() -> String {
    [
        "usage:",
        "  sieve-app auth path",
        "  sieve-app auth login openai-codex",
        "  sieve-app auth set openai-codex --access-token <token> --account-id <id> [--refresh-token <token>] [--expires-at-ms <unix-ms>]",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_set_openai_codex_args_accepts_required_fields() {
        let parsed = parse_set_openai_codex_args(&[
            "--access-token".to_string(),
            "token-123".to_string(),
            "--account-id".to_string(),
            "acc-123".to_string(),
        ])
        .expect("parse set args");
        assert_eq!(parsed.access_token, "token-123");
        assert_eq!(parsed.account_id, "acc-123");
        assert_eq!(parsed.refresh_token, None);
        assert_eq!(parsed.expires_at_ms, None);
    }

    #[test]
    fn parse_set_openai_codex_args_rejects_missing_required_flag() {
        let err = parse_set_openai_codex_args(&["--access-token".to_string(), "token".to_string()])
            .expect_err("must reject missing account id");
        assert!(err.contains("--account-id"));
    }
}
