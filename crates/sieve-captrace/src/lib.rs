#![forbid(unsafe_code)]

mod error;
mod fixture;
mod generator;
mod planner;

#[cfg(test)]
mod tests;

pub use error::CapTraceError;
pub use fixture::{FixtureLayout, TOKEN_IN_FILE, TOKEN_IN_FILE_2, TOKEN_OUT_FILE, TOKEN_TMP_DIR};
pub use generator::{
    derive_summary_from_trace, write_definition_json, BwrapTraceRunner, CapTraceGenerator,
    GenerateDefinitionRequest, GeneratedCommandDefinition, GeneratedSummaryOutcome,
    GeneratedVariantDefinition, TraceRequest, TraceRunner,
};
pub use planner::{CaseGenerationRequest, CaseGenerator, PlannerCaseGenerator};
