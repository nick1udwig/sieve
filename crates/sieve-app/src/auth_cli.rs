use crate::cli::{AuthCommand, AuthProvider};
use reqwest::Client;
use sieve_llm::{
    create_openai_codex_authorization_flow, exchange_openai_codex_authorization_code,
    parse_openai_codex_authorization_input, resolve_openai_codex_auth_json_path_from_env,
    write_openai_codex_auth_file, OpenAiCodexStoredAuth,
};
use std::io::{self, Write};

pub(crate) async fn run_auth_command(command: AuthCommand) -> Result<(), String> {
    match command {
        AuthCommand::Path => {
            println!(
                "{}",
                resolve_openai_codex_auth_json_path_from_env().display()
            );
            Ok(())
        }
        AuthCommand::Login { provider } => run_login_command(provider).await,
        AuthCommand::Set {
            provider,
            access_token,
            account_id,
            refresh_token,
            expires_at_ms,
        } => {
            let auth_path = resolve_openai_codex_auth_json_path_from_env();
            let auth = OpenAiCodexStoredAuth {
                access_token,
                account_id,
                refresh_token,
                expires_at_ms,
            };
            persist_openai_codex_auth(&auth_path, provider, &auth)?;
            println!("saved openai-codex auth to {}", auth_path.display());
            Ok(())
        }
    }
}

async fn run_login_command(provider: AuthProvider) -> Result<(), String> {
    let auth_path = resolve_openai_codex_auth_json_path_from_env();
    let flow = create_openai_codex_authorization_flow(provider_originator(provider))
        .map_err(|err| err.to_string())?;

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
    persist_openai_codex_auth(&auth_path, provider, &auth)?;
    println!("saved openai-codex auth to {}", auth_path.display());
    Ok(())
}

fn persist_openai_codex_auth(
    path: &std::path::Path,
    provider: AuthProvider,
    auth: &OpenAiCodexStoredAuth,
) -> Result<(), String> {
    match provider {
        AuthProvider::OpenAiCodex => write_openai_codex_auth_file(path, auth),
    }
}

fn provider_originator(provider: AuthProvider) -> &'static str {
    match provider {
        AuthProvider::OpenAiCodex => "sieve",
    }
}
