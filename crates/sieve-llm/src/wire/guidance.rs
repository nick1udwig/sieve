use crate::LlmError;
use serde_json::{json, Value};
use sieve_types::{PlannerGuidanceOutput, PlannerGuidanceSignal};

pub(crate) const GUIDANCE_SYSTEM_PROMPT: &str = include_str!("../prompts/guidance_system.md");

pub(crate) fn guidance_output_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "guidance": {
                "type":"object",
                "additionalProperties": false,
                "properties":{
                    "code":{"type":"integer","minimum":0,"maximum":65535},
                    "confidence_bps":{"type":"integer","minimum":0,"maximum":10000},
                    "source_hit_index":{"type":["integer","null"],"minimum":0,"maximum":65535},
                    "evidence_ref_index":{"type":["integer","null"],"minimum":0,"maximum":65535}
                },
                "required":["code","confidence_bps","source_hit_index","evidence_ref_index"]
            }
        },
        "required":["guidance"]
    })
}

pub(crate) fn decode_guidance_output(
    content_json: Value,
) -> Result<PlannerGuidanceOutput, LlmError> {
    let output: PlannerGuidanceOutput = serde_json::from_value(content_json)
        .map_err(|e| LlmError::Decode(format!("invalid guidance output payload: {e}")))?;

    PlannerGuidanceSignal::try_from(output.guidance.code)
        .map_err(|err| LlmError::Boundary(format!("invalid guidance signal: {err}")))?;

    if output.guidance.confidence_bps > 10_000 {
        return Err(LlmError::Boundary(format!(
            "guidance.confidence_bps out of range: {}",
            output.guidance.confidence_bps
        )));
    }

    Ok(output)
}
