use crate::LlmError;
use serde_json::Value;

pub(crate) fn extract_openai_message_content_json(response: &Value) -> Result<Value, LlmError> {
    ensure_not_refusal(response)?;

    let content = extract_chat_completions_message_text(response)
        .or_else(|| extract_responses_message_text(response))
        .ok_or_else(|| {
            LlmError::Decode(
                "missing choices[0].message.content string or responses output_text".to_string(),
            )
        })?;

    serde_json::from_str::<Value>(&content)
        .map_err(|e| LlmError::Decode(format!("content is not valid JSON object: {e}")))
}

fn extract_chat_completions_message_text(response: &Value) -> Option<String> {
    let content = response.pointer("/choices/0/message/content")?;
    match content {
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        Value::Array(parts) => collect_message_text_parts(parts),
        _ => None,
    }
}

pub(super) fn ensure_not_refusal(response: &Value) -> Result<(), LlmError> {
    if let Some(refusal) = response
        .pointer("/choices/0/message/refusal")
        .and_then(Value::as_str)
    {
        return Err(LlmError::Backend(format!(
            "model refused request: {refusal}"
        )));
    }
    if let Some(refusal) = extract_responses_refusal(response) {
        return Err(LlmError::Backend(format!(
            "model refused request: {refusal}"
        )));
    }
    Ok(())
}

fn extract_responses_message_text(response: &Value) -> Option<String> {
    let output = response.get("output")?.as_array()?;
    let mut texts = Vec::new();

    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
            if part_type == "output_text" || part_type == "text" {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        texts.push(text.to_string());
                    }
                }
            }
        }
    }

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

fn collect_message_text_parts(parts: &[Value]) -> Option<String> {
    let mut texts = Vec::new();

    for part in parts {
        if let Some(text) = part.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                texts.push(text.to_string());
            }
            continue;
        }

        let part_type = part.get("type").and_then(Value::as_str).unwrap_or_default();
        if (part_type == "output_text" || part_type == "text" || part_type.is_empty())
            && part.get("text").and_then(Value::as_str).is_some()
        {
            let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
            let text = text.trim();
            if !text.is_empty() {
                texts.push(text.to_string());
            }
        }
    }

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

fn extract_responses_refusal(response: &Value) -> Option<String> {
    if let Some(refusal) = response.get("refusal").and_then(Value::as_str) {
        let refusal = refusal.trim();
        if !refusal.is_empty() {
            return Some(refusal.to_string());
        }
    }

    let output = response.get("output")?.as_array()?;
    for item in output {
        let Some(content) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in content {
            if let Some(refusal) = part.get("refusal").and_then(Value::as_str) {
                let refusal = refusal.trim();
                if !refusal.is_empty() {
                    return Some(refusal.to_string());
                }
            }
            if part.get("type").and_then(Value::as_str) == Some("refusal") {
                if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| part.get("refusal").and_then(Value::as_str))
                {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }
    None
}
