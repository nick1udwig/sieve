mod guidance;
mod openai_envelope;
mod planner;
mod response;

pub(crate) use guidance::{decode_guidance_output, guidance_output_schema, GUIDANCE_SYSTEM_PROMPT};
pub(crate) use openai_envelope::extract_openai_message_content_json;
pub(crate) use planner::{
    decode_planner_output, extract_openai_planner_output_json,
    planner_regeneration_diagnostic_prompt, serialize_planner_input, PlannerDecodeOutcome,
    PLANNER_SYSTEM_PROMPT,
};
pub(crate) use response::{
    decode_response_output, response_output_schema, serialize_response_input,
    RESPONSE_SYSTEM_PROMPT,
};
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use sieve_types::{PlannerGuidanceSignal, PlannerTurnInput};

#[cfg(test)]
mod tests;
