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
    derive_summary_from_trace, render_rust_snippet, write_definition_json, BwrapTraceRunner,
    CapTraceGenerator, GenerateDefinitionRequest, GeneratedCommandDefinition,
    GeneratedSummaryOutcome, GeneratedVariantDefinition, TraceRequest, TraceRunner,
};
pub use planner::{
    preferred_case_generator_from_env, CaseGenerationRequest, CaseGenerator, CaseGeneratorBackend,
    PlannerCaseGenerator,
};
