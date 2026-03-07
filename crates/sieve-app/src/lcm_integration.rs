#![forbid(unsafe_code)]

use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};
use tokio::process::Command as TokioCommand;

#[derive(Clone)]
pub struct LcmIntegrationConfig {
    pub enabled: bool,
    pub global_session_id: String,
    pub trusted_db_path: PathBuf,
    pub untrusted_db_path: PathBuf,
    pub cli_bin: String,
}

impl LcmIntegrationConfig {
    pub fn from_sieve_home(sieve_home: &Path) -> Self {
        let trusted_db_path = env::var("SIEVE_LCM_TRUSTED_DB_PATH")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| sieve_home.join("lcm/trusted.db"));
        let untrusted_db_path = env::var("SIEVE_LCM_UNTRUSTED_DB_PATH")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| sieve_home.join("lcm/untrusted.db"));
        let global_session_id = env::var("SIEVE_LCM_GLOBAL_SESSION_ID")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| "global".to_string());
        let cli_bin = env::var("SIEVE_LCM_CLI_BIN")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| "sieve-lcm-cli".to_string());

        Self {
            enabled: parse_bool_env("SIEVE_LCM_ENABLED", true),
            global_session_id,
            trusted_db_path,
            untrusted_db_path,
            cli_bin,
        }
    }
}

#[derive(Clone)]
pub struct LcmIntegration {
    config: LcmIntegrationConfig,
}

impl LcmIntegration {
    pub fn new(config: LcmIntegrationConfig) -> Result<Self, String> {
        if !config.enabled {
            return Err("lcm integration disabled".to_string());
        }
        Ok(Self { config })
    }

    pub async fn ingest_user_message_for_session(
        &self,
        session_key: &str,
        message: &str,
    ) -> Result<(), String> {
        if message.trim().is_empty() {
            return Ok(());
        }

        let conversation = self.conversation_id_for_session(session_key);
        self.ingest_lane(
            "trusted",
            &self.config.trusted_db_path,
            &conversation,
            "user",
            message,
        )
        .await?;
        self.ingest_lane(
            "untrusted",
            &self.config.untrusted_db_path,
            &conversation,
            "user",
            message,
        )
        .await
    }

    pub async fn ingest_assistant_message_for_session(
        &self,
        session_key: &str,
        message: &str,
    ) -> Result<(), String> {
        if message.trim().is_empty() {
            return Ok(());
        }

        let conversation = self.conversation_id_for_session(session_key);
        self.ingest_lane(
            "untrusted",
            &self.config.untrusted_db_path,
            &conversation,
            "assistant",
            message,
        )
        .await
    }

    fn conversation_id_for_session(&self, session_key: &str) -> String {
        let trimmed = session_key.trim();
        if trimmed.is_empty() || trimmed == "main" {
            return self.config.global_session_id.clone();
        }
        format!("{}:{trimmed}", self.config.global_session_id)
    }

    async fn ingest_lane(
        &self,
        lane_name: &str,
        db_path: &Path,
        conversation: &str,
        role: &str,
        content: &str,
    ) -> Result<(), String> {
        let output = TokioCommand::new(&self.config.cli_bin)
            .arg("ingest")
            .arg("--db")
            .arg(db_path)
            .arg("--conversation")
            .arg(conversation)
            .arg("--role")
            .arg(role)
            .arg("--content")
            .arg(content)
            .arg("--json")
            .output()
            .await
            .map_err(|err| {
                format!(
                    "lcm ingest spawn failed on {lane_name} lane via `{}`: {err}",
                    self.config.cli_bin
                )
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            return Err(format!(
                "lcm ingest failed on {lane_name} lane: {}",
                if detail.is_empty() {
                    "unknown error".to_string()
                } else {
                    detail
                }
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            return Err(format!(
                "lcm ingest returned empty stdout on {lane_name} lane"
            ));
        }

        let payload: Value = serde_json::from_str(&stdout).map_err(|err| {
            format!("lcm ingest returned invalid json on {lane_name} lane: {err}")
        })?;
        if payload.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(format!(
                "lcm ingest returned non-ok payload on {lane_name} lane: {}",
                payload
            ));
        }

        Ok(())
    }
}

fn parse_bool_env(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}
