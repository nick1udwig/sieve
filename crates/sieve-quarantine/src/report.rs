use crate::QuarantineRunError;
use sieve_types::{Action, QuarantineReport, Resource};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const REPORT_FILE_NAME: &str = "report.json";

pub(crate) fn write_report_json(
    run_dir: &Path,
    trace_files: &[PathBuf],
    report: &QuarantineReport,
) -> Result<(), QuarantineRunError> {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str(&format!(
        "  \"run_id\": {},\n",
        json_string(&report.run_id.0)
    ));
    json.push_str(&format!(
        "  \"trace_path\": {},\n",
        json_string(&report.trace_path)
    ));
    json.push_str(&format!(
        "  \"stdout_path\": {},\n",
        json_optional_string(report.stdout_path.as_deref())
    ));
    json.push_str(&format!(
        "  \"stderr_path\": {},\n",
        json_optional_string(report.stderr_path.as_deref())
    ));

    json.push_str("  \"trace_files\": [");
    if trace_files.is_empty() {
        json.push_str("],\n");
    } else {
        json.push('\n');
        for (index, trace_file) in trace_files.iter().enumerate() {
            let comma = if index + 1 < trace_files.len() { "," } else { "" };
            let trace_file = trace_file.to_string_lossy();
            json.push_str(&format!("    {}{comma}\n", json_string(&trace_file)));
        }
        json.push_str("  ],\n");
    }

    json.push_str("  \"attempted_capabilities\": [");
    if report.attempted_capabilities.is_empty() {
        json.push_str("],\n");
    } else {
        json.push('\n');
        for (index, capability) in report.attempted_capabilities.iter().enumerate() {
            let comma = if index + 1 < report.attempted_capabilities.len() {
                ","
            } else {
                ""
            };
            json.push_str("    {\n");
            json.push_str(&format!(
                "      \"resource\": {},\n",
                json_string(resource_name(capability.resource))
            ));
            json.push_str(&format!(
                "      \"action\": {},\n",
                json_string(action_name(capability.action))
            ));
            json.push_str(&format!(
                "      \"scope\": {}\n",
                json_string(&capability.scope)
            ));
            json.push_str(&format!("    }}{comma}\n"));
        }
        json.push_str("  ],\n");
    }

    json.push_str(&format!(
        "  \"exit_code\": {}\n",
        report
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "null".to_string())
    ));
    json.push_str("}\n");

    fs::write(run_dir.join(REPORT_FILE_NAME), json).map_err(io_err)
}

pub(crate) fn io_err(err: io::Error) -> QuarantineRunError {
    QuarantineRunError::Exec(err.to_string())
}

pub(crate) fn truncate_bytes_for_error(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn json_string(value: &str) -> String {
    format!("\"{}\"", json_escape(value))
}

fn json_optional_string(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_string())
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => escaped.push_str(&format!("\\u{:04x}", c as u32)),
            c => escaped.push(c),
        }
    }
    escaped
}

fn resource_name(resource: Resource) -> &'static str {
    match resource {
        Resource::Fs => "fs",
        Resource::Net => "net",
        Resource::Proc => "proc",
        Resource::Env => "env",
        Resource::Ipc => "ipc",
    }
}

fn action_name(action: Action) -> &'static str {
    match action {
        Action::Read => "read",
        Action::Write => "write",
        Action::Append => "append",
        Action::Exec => "exec",
        Action::Connect => "connect",
    }
}
