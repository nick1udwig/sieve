use crate::LlmError;
use serde_json::Value;

pub(crate) fn extract_openai_message_content_json(response: &Value) -> Result<Value, LlmError> {
    ensure_not_refusal(response)?;

    let content = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or_else(|| LlmError::Decode("missing choices[0].message.content string".to_string()))?;

    serde_json::from_str::<Value>(content)
        .map_err(|e| LlmError::Decode(format!("content is not valid JSON object: {e}")))
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
    Ok(())
}
