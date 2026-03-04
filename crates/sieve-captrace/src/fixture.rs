#![forbid(unsafe_code)]

use crate::error::{io_err, CapTraceError};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const TOKEN_TMP_DIR: &str = "{{TMP_DIR}}";
pub const TOKEN_IN_FILE: &str = "{{IN_FILE}}";
pub const TOKEN_IN_FILE_2: &str = "{{IN_FILE_2}}";
pub const TOKEN_OUT_FILE: &str = "{{OUT_FILE}}";
pub const TOKEN_URL: &str = "{{URL}}";
pub const TOKEN_HEADER: &str = "{{HEADER}}";
pub const TOKEN_DATA: &str = "{{DATA}}";
pub const TOKEN_KV: &str = "{{KV}}";
pub const TOKEN_ARG: &str = "{{ARG}}";

#[derive(Debug, Clone)]
pub struct FixtureLayout {
    pub root: PathBuf,
    pub in_file: PathBuf,
    pub in_file_2: PathBuf,
    pub out_file: PathBuf,
}

impl FixtureLayout {
    pub fn apply_to_argv_template(&self, argv_template: &[String]) -> Vec<String> {
        argv_template
            .iter()
            .map(|arg| self.apply_tokens(arg))
            .collect()
    }

    pub fn normalize_scope_for_definition(&self, scope: &str) -> String {
        if scope == self.in_file.to_string_lossy() {
            return TOKEN_IN_FILE.to_string();
        }
        if scope == self.in_file_2.to_string_lossy() {
            return TOKEN_IN_FILE_2.to_string();
        }
        if scope == self.out_file.to_string_lossy() {
            return TOKEN_OUT_FILE.to_string();
        }
        let root = self.root.to_string_lossy();
        if scope == root {
            return TOKEN_TMP_DIR.to_string();
        }
        if let Some(suffix) = scope.strip_prefix(root.as_ref()) {
            if suffix.starts_with('/') {
                return format!("{TOKEN_TMP_DIR}{suffix}");
            }
            return format!("{TOKEN_TMP_DIR}/{suffix}");
        }
        scope.to_string()
    }

    fn apply_tokens(&self, value: &str) -> String {
        let tmp_dir = self.root.to_string_lossy();
        let in_file = self.in_file.to_string_lossy();
        let in_file_2 = self.in_file_2.to_string_lossy();
        let out_file = self.out_file.to_string_lossy();

        value
            .replace(TOKEN_TMP_DIR, tmp_dir.as_ref())
            .replace(TOKEN_IN_FILE, in_file.as_ref())
            .replace(TOKEN_IN_FILE_2, in_file_2.as_ref())
            .replace(TOKEN_OUT_FILE, out_file.as_ref())
            .replace(TOKEN_URL, "https://example.com/resource")
            .replace(TOKEN_HEADER, "Authorization: Bearer token")
            .replace(TOKEN_DATA, "payload=example")
            .replace(TOKEN_KV, "key=value")
            .replace(TOKEN_ARG, "example")
    }
}

pub fn create_fixture_layout() -> Result<FixtureLayout, CapTraceError> {
    let base = std::env::temp_dir().join(format!(
        "sieve-captrace-fixture-{}-{}",
        std::process::id(),
        now_ms()
    ));
    fs::create_dir_all(&base).map_err(io_err)?;

    let in_file = base.join("input.txt");
    let in_file_2 = base.join("input-2.txt");
    let out_file = base.join("output.txt");

    fs::write(&in_file, b"fixture-input\n").map_err(io_err)?;
    fs::write(&in_file_2, b"fixture-input-2\n").map_err(io_err)?;
    fs::write(&out_file, b"fixture-output\n").map_err(io_err)?;

    Ok(FixtureLayout {
        root: base,
        in_file,
        in_file_2,
        out_file,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
