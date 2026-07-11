use crate::auth_cli::run_auth_command;
use crate::cli::{AuthCommand, AuthProvider};
use crate::config::parse_sieve_home;
use crate::host;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

const DEFAULT_MODEL: &str = "gpt-5.4";
const MODEL_ROLES: &[&str] = &["PLANNER", "GUIDANCE", "RESPONSE", "QUARANTINE"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelProvider {
    OpenAiApi,
    OpenAiCodex,
}

impl ModelProvider {
    fn env_value(self) -> &'static str {
        match self {
            Self::OpenAiApi => "openai",
            Self::OpenAiCodex => "openai_codex",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::OpenAiApi => "OpenAI API key",
            Self::OpenAiCodex => "OpenAI Codex subscription",
        }
    }
}

pub(crate) async fn run_setup_command(
    non_interactive: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Sieve setup");
    let capabilities = host::RuntimeCapabilityManifest::current();
    capabilities
        .report()
        .print(false)
        .map_err(io::Error::other)?;
    offer_host_dependencies(non_interactive, capabilities)?;

    if non_interactive || !io::stdin().is_terminal() {
        println!();
        println!("Configure a model provider interactively with `sieve setup`:");
        println!("  OpenAI API key (pay-as-you-go)");
        println!("  OpenAI Codex (ChatGPT subscription)");
        return Ok(());
    }

    println!();
    println!("Model provider");
    println!("  1) OpenAI API key — pay-as-you-go API billing");
    println!("  2) OpenAI Codex — use a ChatGPT subscription");
    println!("  3) Keep current configuration / configure later");
    if let Some(current) = configured_provider() {
        println!("Current: {}", current.label());
    }
    let default = match configured_provider() {
        Some(ModelProvider::OpenAiCodex) => 2,
        _ => 1,
    };
    let selection = prompt_line(&format!("Select provider [{default}]: "))?;
    let selection = if selection.trim().is_empty() {
        default
    } else {
        match selection.trim().parse::<u8>() {
            Ok(value @ 1..=3) => value,
            _ => {
                println!("Invalid selection; configuration unchanged.");
                return Ok(());
            }
        }
    };

    match selection {
        1 => configure_openai_api().await?,
        2 => configure_openai_codex().await?,
        _ => println!("Configuration unchanged."),
    }
    Ok(())
}

fn offer_host_dependencies(
    non_interactive: bool,
    capabilities: &host::RuntimeCapabilityManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(plan) = capabilities.recommended_install_plan() else {
        return Ok(());
    };
    println!();
    println!(
        "Recommended host tools missing: {}",
        plan.missing_features.join(", ")
    );
    println!("Proposed command: {}", plan.display_command());
    if non_interactive || !io::stdin().is_terminal() {
        return Ok(());
    }
    let answer = prompt_line("Install these host tools now? [y/N] ")?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        plan.execute().map_err(io::Error::other)?;
        println!("Host tools installed.");
    }
    Ok(())
}

async fn configure_openai_api() -> Result<(), Box<dyn std::error::Error>> {
    let existing = non_empty_env("OPENAI_API_KEY");
    let api_key = if let Some(existing) = existing {
        println!("OpenAI API key: {} ✓", mask_secret(&existing));
        let action = prompt_line("[K]eep / [R]eplace / [B]ack (default K): ")?;
        match action.trim().to_ascii_lowercase().as_str() {
            "r" | "replace" => prompt_api_key()?,
            "b" | "back" => return Ok(()),
            _ => existing,
        }
    } else {
        println!("No OpenAI API key configured.");
        prompt_api_key()?
    };
    if api_key.is_empty() {
        println!("No key entered; configuration unchanged.");
        return Ok(());
    }
    let model = prompt_model()?;
    let path = persistent_env_path();
    write_provider_config(
        &path,
        ModelProvider::OpenAiApi.env_value(),
        &model,
        Some(&api_key),
    )?;
    println!("Saved OpenAI API configuration to {}.", path.display());
    println!("Setup complete; run `sieve doctor` to verify it.");
    Ok(())
}

async fn configure_openai_codex() -> Result<(), Box<dyn std::error::Error>> {
    let auth_path = sieve_llm::resolve_openai_codex_auth_json_path_from_env();
    let should_login = if auth_path.is_file() {
        println!("OpenAI Codex credentials found at {}.", auth_path.display());
        let action = prompt_line("[K]eep / [R]eauthenticate / [B]ack (default K): ")?;
        match action.trim().to_ascii_lowercase().as_str() {
            "r" | "reauthenticate" => true,
            "b" | "back" => return Ok(()),
            _ => false,
        }
    } else {
        println!("OpenAI Codex login uses your ChatGPT subscription.");
        true
    };
    if should_login {
        run_auth_command(AuthCommand::Login {
            provider: AuthProvider::OpenAiCodex,
        })
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    }
    let model = prompt_model()?;
    let path = persistent_env_path();
    write_provider_config(&path, ModelProvider::OpenAiCodex.env_value(), &model, None)?;
    println!("Saved OpenAI Codex configuration to {}.", path.display());
    println!("Setup complete; run `sieve doctor` to verify it.");
    Ok(())
}

fn prompt_api_key() -> Result<String, Box<dyn std::error::Error>> {
    let value = match rpassword::prompt_password("OpenAI API key (input hidden, Enter to cancel): ")
    {
        Ok(value) => value,
        Err(err) => {
            eprintln!("Hidden terminal input unavailable ({err}); input will be visible.");
            prompt_line("OpenAI API key (Enter to cancel): ")?
        }
    };
    Ok(value.trim().to_string())
}

fn prompt_model() -> io::Result<String> {
    let current = non_empty_env("SIEVE_PLANNER_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let entered = prompt_line(&format!("Model [{current}]: "))?;
    if entered.trim().is_empty() {
        Ok(current)
    } else {
        Ok(entered.trim().to_string())
    }
}

fn prompt_line(message: &str) -> io::Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer)
}

fn configured_provider() -> Option<ModelProvider> {
    match non_empty_env("SIEVE_PLANNER_PROVIDER")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "openai_codex" | "openai-codex" => Some(ModelProvider::OpenAiCodex),
        "openai" => Some(ModelProvider::OpenAiApi),
        _ if non_empty_env("OPENAI_API_KEY").is_some() => Some(ModelProvider::OpenAiApi),
        _ if sieve_llm::resolve_openai_codex_auth_json_path_from_env().is_file() => {
            Some(ModelProvider::OpenAiCodex)
        }
        _ => None,
    }
}

fn persistent_env_path() -> PathBuf {
    parse_sieve_home(env::var("SIEVE_HOME").ok(), env::var("HOME").ok()).join(".env")
}

fn write_provider_config(
    path: &Path,
    provider: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    let mut values = BTreeMap::new();
    for role in MODEL_ROLES {
        values.insert(format!("SIEVE_{role}_PROVIDER"), provider.to_string());
    }
    values.insert("SIEVE_PLANNER_MODEL".to_string(), model.to_string());
    if let Some(api_key) = api_key {
        values.insert("OPENAI_API_KEY".to_string(), api_key.to_string());
    }
    upsert_dotenv_values(path, &values)
}

fn upsert_dotenv_values(path: &Path, values: &BTreeMap<String, String>) -> Result<(), String> {
    for (key, value) in values {
        if key.contains(['\n', '\r', '=']) || value.contains(['\n', '\r', '\0']) {
            return Err(format!("invalid dotenv value for {key}"));
        }
    }
    let existing = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("failed reading {}: {err}", path.display())),
    };
    let mut remaining = values.clone();
    let mut lines = Vec::new();
    for line in existing.lines() {
        let key = line.split_once('=').map(|(key, _)| key.trim());
        if let Some(value) = key.and_then(|key| remaining.remove(key)) {
            lines.push(format!("{}={value}", key.expect("dotenv key missing")));
        } else if key.is_some_and(|key| values.contains_key(key)) {
            continue;
        } else {
            lines.push(line.to_string());
        }
    }
    lines.extend(
        remaining
            .into_iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    let body = format!("{}\n", lines.join("\n"));
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("failed creating {}: {err}", parent.display()))?;
    let tmp = parent.join(format!(".env.tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|err| format!("failed opening {}: {err}", tmp.display()))?;
    file.write_all(body.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|err| format!("failed writing {}: {err}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|err| format!("failed replacing {}: {err}", path.display()))
}

fn mask_secret(value: &str) -> String {
    let prefix = value.chars().take(8).collect::<String>();
    format!("{prefix}…")
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_env_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("sieve-setup-{label}-{}", std::process::id()))
            .join(".env")
    }

    #[test]
    fn api_key_provider_config_is_persistent_and_preserves_unrelated_values() {
        let path = temp_env_path("api-key");
        fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
        fs::write(&path, "UNRELATED=value\nSIEVE_PLANNER_PROVIDER=old\n").expect("seed env");

        write_provider_config(&path, "openai", "gpt-5.4", Some("sk-test"))
            .expect("write provider config");

        let body = fs::read_to_string(&path).expect("read env");
        assert!(body.contains("UNRELATED=value\n"));
        assert!(body.contains("OPENAI_API_KEY=sk-test\n"));
        assert!(body.contains("SIEVE_PLANNER_PROVIDER=openai\n"));
        assert!(body.contains("SIEVE_PLANNER_MODEL=gpt-5.4\n"));
        assert_eq!(body.matches("SIEVE_PLANNER_PROVIDER=").count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn codex_provider_config_sets_every_model_role_without_erasing_api_key() {
        let path = temp_env_path("codex");
        fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
        fs::write(&path, "OPENAI_API_KEY=existing\n").expect("seed env");

        write_provider_config(&path, "openai_codex", "gpt-5.4", None)
            .expect("write provider config");

        let body = fs::read_to_string(&path).expect("read env");
        assert!(body.contains("OPENAI_API_KEY=existing\n"));
        for role in ["PLANNER", "GUIDANCE", "RESPONSE", "QUARANTINE"] {
            assert!(body.contains(&format!("SIEVE_{role}_PROVIDER=openai_codex\n")));
        }
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }
}
