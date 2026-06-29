use crate::error::CapTraceError;
use serde_json::Value;

pub(super) fn parse_cases_from_agent_message(raw: &str) -> Result<Vec<String>, CapTraceError> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return cases_from_json(value);
    }

    let Some(start) = raw.find('{') else {
        return Err(CapTraceError::Llm(
            "codex app-server returned non-JSON output".to_string(),
        ));
    };
    let Some(end) = raw.rfind('}') else {
        return Err(CapTraceError::Llm(
            "codex app-server returned malformed JSON output".to_string(),
        ));
    };

    let sliced = &raw[start..=end];
    let value = serde_json::from_str::<Value>(sliced).map_err(|err| {
        CapTraceError::Llm(format!("failed to parse app-server JSON output: {err}"))
    })?;
    cases_from_json(value)
}

fn cases_from_json(value: Value) -> Result<Vec<String>, CapTraceError> {
    let Some(cases) = value.get("cases").and_then(Value::as_array) else {
        return Err(CapTraceError::Llm(
            "codex app-server output missing `cases` array".to_string(),
        ));
    };

    let out: Vec<String> = cases
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect();
    if out.is_empty() {
        return Err(CapTraceError::Llm(
            "codex app-server returned empty `cases`".to_string(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::parse_cases_from_agent_message;

    #[test]
    fn parses_cases_from_exact_json() {
        let cases =
            parse_cases_from_agent_message(r#"{"cases":["ls -la","ls src"]}"#).expect("cases");
        assert_eq!(cases, vec!["ls -la", "ls src"]);
    }

    #[test]
    fn parses_cases_from_prefixed_json_block() {
        let cases =
            parse_cases_from_agent_message("```json\n{\"cases\":[\"cat Cargo.toml\"]}\n```")
                .expect("cases");
        assert_eq!(cases, vec!["cat Cargo.toml"]);
    }

    #[test]
    fn rejects_missing_cases_array() {
        let err =
            parse_cases_from_agent_message(r#"{"commands":["ls"]}"#).expect_err("missing cases");
        assert!(err
            .to_string()
            .contains("codex app-server output missing `cases` array"));
    }
}
