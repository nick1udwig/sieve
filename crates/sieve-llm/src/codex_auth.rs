use crate::LlmError;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use url::Url;
use uuid::Uuid;

pub const OPENAI_CODEX_AUTH_PATH_ENV: &str = "SIEVE_OPENAI_CODEX_AUTH_JSON_PATH";
pub const OPENAI_CODEX_ACCESS_TOKEN_ENV: &str = "OPENAI_CODEX_ACCESS_TOKEN";
pub const OPENAI_CODEX_ACCOUNT_ID_ENV: &str = "OPENAI_CODEX_ACCOUNT_ID";
pub const OPENAI_CODEX_PROVIDER_ID: &str = "openai-codex";

const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_CODEX_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_CODEX_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const OPENAI_CODEX_SCOPE: &str = "openid profile email offline_access";
const OPENAI_CODEX_JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";
const SIEVE_AUTH_DIR_SUFFIX: &str = ".sieve/state/auth.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCodexStoredAuth {
    pub access_token: String,
    pub account_id: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCodexAuthorizationFlow {
    pub authorization_url: String,
    pub verifier: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCodexAuthorizationInput {
    pub code: String,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCodexTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

pub fn default_openai_codex_auth_json_path(
    raw_sieve_home: Option<String>,
    raw_home: Option<String>,
) -> PathBuf {
    if let Some(sieve_home) = trim_to_non_empty(raw_sieve_home) {
        return PathBuf::from(sieve_home).join("state/auth.json");
    }
    if let Some(home) = trim_to_non_empty(raw_home) {
        return PathBuf::from(home).join(SIEVE_AUTH_DIR_SUFFIX);
    }
    PathBuf::from(SIEVE_AUTH_DIR_SUFFIX)
}

pub fn resolve_openai_codex_auth_json_path(getter: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    trim_to_non_empty(getter(OPENAI_CODEX_AUTH_PATH_ENV))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            default_openai_codex_auth_json_path(getter("SIEVE_HOME"), getter("HOME"))
        })
}

pub fn resolve_openai_codex_auth_json_path_from_env() -> PathBuf {
    resolve_openai_codex_auth_json_path(&|key| env::var(key).ok())
}

pub fn parse_openai_codex_authorization_input(
    input: &str,
) -> Result<OpenAiCodexAuthorizationInput, LlmError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(LlmError::Config(
            "missing authorization code or redirect URL".to_string(),
        ));
    }

    if let Ok(url) = Url::parse(trimmed) {
        let code = url
            .query_pairs()
            .find_map(|(key, value)| (key == "code").then_some(value.into_owned()))
            .filter(|value| !value.trim().is_empty());
        let state = url
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then_some(value.into_owned()))
            .filter(|value| !value.trim().is_empty());
        if let Some(code) = code {
            return Ok(OpenAiCodexAuthorizationInput { code, state });
        }
    }

    if trimmed.contains("code=") {
        let mut code = None;
        let mut state = None;
        for (key, value) in url::form_urlencoded::parse(trimmed.as_bytes()) {
            match key.as_ref() {
                "code" if !value.trim().is_empty() => code = Some(value.into_owned()),
                "state" if !value.trim().is_empty() => state = Some(value.into_owned()),
                _ => {}
            }
        }
        if let Some(code) = code {
            return Ok(OpenAiCodexAuthorizationInput { code, state });
        }
    }

    Ok(OpenAiCodexAuthorizationInput {
        code: trimmed.to_string(),
        state: None,
    })
}

pub fn create_openai_codex_authorization_flow(
    originator: &str,
) -> Result<OpenAiCodexAuthorizationFlow, LlmError> {
    let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = Uuid::new_v4().simple().to_string();
    let mut url = Url::parse(OPENAI_CODEX_AUTHORIZE_URL)
        .map_err(|err| LlmError::Config(format!("invalid OpenAI authorize URL: {err}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", OPENAI_CODEX_CLIENT_ID)
        .append_pair("redirect_uri", OPENAI_CODEX_REDIRECT_URI)
        .append_pair("scope", OPENAI_CODEX_SCOPE)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state)
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", originator);
    Ok(OpenAiCodexAuthorizationFlow {
        authorization_url: url.to_string(),
        verifier,
        state,
    })
}

pub async fn exchange_openai_codex_authorization_code(
    http: &Client,
    code: &str,
    verifier: &str,
) -> Result<OpenAiCodexStoredAuth, LlmError> {
    let response = http
        .post(OPENAI_CODEX_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", OPENAI_CODEX_REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|err| {
            LlmError::Transport(format!(
                "openai_codex authorization code exchange failed: {err}"
            ))
        })?;
    parse_openai_codex_token_response(response).await
}

pub async fn refresh_openai_codex_access_token(
    http: &Client,
    refresh_token: &str,
) -> Result<OpenAiCodexStoredAuth, LlmError> {
    let response = http
        .post(OPENAI_CODEX_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
        ])
        .send()
        .await
        .map_err(|err| {
            LlmError::Transport(format!("openai_codex token refresh request failed: {err}"))
        })?;
    parse_openai_codex_token_response(response).await
}

pub fn read_openai_codex_auth_file(path: &Path) -> Result<OpenAiCodexStoredAuth, LlmError> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(LlmError::Config(format!(
                "missing `{}`; run `sieve-app auth login openai-codex` or `sieve-app auth set openai-codex`",
                path.display()
            )))
        }
        Err(err) => {
            return Err(LlmError::Config(format!(
                "failed reading `{}`: {err}",
                path.display()
            )))
        }
    };
    let auth_json: Value = serde_json::from_str(&raw)
        .map_err(|err| LlmError::Decode(format!("invalid JSON in `{}`: {err}", path.display())))?;
    let entry = auth_json
        .get(OPENAI_CODEX_PROVIDER_ID)
        .ok_or_else(|| {
            LlmError::Config(format!(
                "missing `{OPENAI_CODEX_PROVIDER_ID}` in `{}`; run `sieve-app auth login openai-codex` or `sieve-app auth set openai-codex`",
                path.display()
            ))
        })?
        .as_object()
        .ok_or_else(|| {
            LlmError::Decode(format!(
                "`{OPENAI_CODEX_PROVIDER_ID}` entry in `{}` must be an object",
                path.display()
            ))
        })?;

    let entry_type = entry
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if entry_type != "oauth" {
        return Err(LlmError::Config(format!(
            "`{OPENAI_CODEX_PROVIDER_ID}` entry in `{}` must have `type: oauth`",
            path.display()
        )));
    }

    let access_token = entry
        .get("access")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            LlmError::Config(format!(
                "`{OPENAI_CODEX_PROVIDER_ID}` entry in `{}` is missing `access`",
                path.display()
            ))
        })?
        .to_string();
    let account_id = entry
        .get("accountId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            LlmError::Config(format!(
                "`{OPENAI_CODEX_PROVIDER_ID}` entry in `{}` is missing `accountId`",
                path.display()
            ))
        })?
        .to_string();
    let refresh_token = entry
        .get("refresh")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let expires_at_ms = entry.get("expires").and_then(Value::as_u64);

    Ok(OpenAiCodexStoredAuth {
        access_token,
        account_id,
        refresh_token,
        expires_at_ms,
    })
}

pub fn write_openai_codex_auth_file(
    path: &Path,
    auth: &OpenAiCodexStoredAuth,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating {}: {err}", parent.display()))?;
    }

    let mut root = match fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|err| format!("failed parsing {}: {err}", path.display()))?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(err) => return Err(format!("failed reading {}: {err}", path.display())),
    };

    let root_map = root
        .as_object_mut()
        .ok_or_else(|| format!("auth file {} root must be a JSON object", path.display()))?;

    let mut entry = Map::new();
    entry.insert("type".to_string(), Value::String("oauth".to_string()));
    entry.insert(
        "access".to_string(),
        Value::String(auth.access_token.clone()),
    );
    entry.insert(
        "accountId".to_string(),
        Value::String(auth.account_id.clone()),
    );
    if let Some(refresh_token) = auth.refresh_token.as_ref() {
        entry.insert("refresh".to_string(), Value::String(refresh_token.clone()));
    }
    if let Some(expires_at_ms) = auth.expires_at_ms {
        entry.insert("expires".to_string(), json!(expires_at_ms));
    }

    root_map.insert(OPENAI_CODEX_PROVIDER_ID.to_string(), Value::Object(entry));

    let serialized = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("failed serializing {}: {err}", path.display()))?;
    let tmp_path = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp_path, serialized)
        .map_err(|err| format!("failed writing {}: {err}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).map_err(|err| {
        format!(
            "failed renaming {} to {}: {err}",
            tmp_path.display(),
            path.display()
        )
    })
}

async fn parse_openai_codex_token_response(
    response: reqwest::Response,
) -> Result<OpenAiCodexStoredAuth, LlmError> {
    let status = response.status();
    let body = response.text().await.map_err(|err| {
        LlmError::Transport(format!(
            "failed reading openai_codex token response body: {err}"
        ))
    })?;

    if !status.is_success() {
        return Err(LlmError::Backend(format!(
            "openai_codex token request failed with status {}: {}",
            status.as_u16(),
            truncate_for_error(&body)
        )));
    }

    let parsed: OpenAiCodexTokenResponse = serde_json::from_str(&body)
        .map_err(|err| LlmError::Decode(format!("invalid openai_codex token JSON: {err}")))?;
    let access_token = parsed.access_token.ok_or_else(|| {
        LlmError::Decode("openai_codex token response missing access_token".to_string())
    })?;
    let refresh_token = parsed.refresh_token.ok_or_else(|| {
        LlmError::Decode("openai_codex token response missing refresh_token".to_string())
    })?;
    let expires_in = parsed.expires_in.ok_or_else(|| {
        LlmError::Decode("openai_codex token response missing expires_in".to_string())
    })?;
    let account_id = extract_account_id(&access_token).ok_or_else(|| {
        LlmError::Decode("failed extracting chatgpt account id from access token".to_string())
    })?;

    Ok(OpenAiCodexStoredAuth {
        access_token,
        account_id,
        refresh_token: Some(refresh_token),
        expires_at_ms: Some(now_ms().saturating_add(expires_in.saturating_mul(1000))),
    })
}

fn extract_account_id(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    let json: Value = serde_json::from_slice(&decoded).ok()?;
    json.get(OPENAI_CODEX_JWT_CLAIM_PATH)?
        .get("chatgpt_account_id")?
        .as_str()
        .map(ToString::to_string)
}

fn trim_to_non_empty(raw: Option<String>) -> Option<String> {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn truncate_for_error(input: &str) -> String {
    const MAX: usize = 512;
    if input.len() <= MAX {
        input.to_string()
    } else {
        format!("{}...[truncated]", &input[..MAX])
    }
}
