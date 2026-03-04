#![forbid(unsafe_code)]

mod error;
mod fixture;
mod generator;
mod planner;

#[cfg(test)]
mod tests;

pub use error::CapTraceError;
pub use fixture::{
    FixtureLayout, TOKEN_ARG, TOKEN_DATA, TOKEN_HEADER, TOKEN_IN_FILE, TOKEN_IN_FILE_2, TOKEN_KV,
    TOKEN_OUT_FILE, TOKEN_TMP_DIR, TOKEN_URL,
};
pub use generator::{
    derive_summary_from_trace, render_rust_snippet, write_definition_json, BwrapTraceRunner,
    CapTraceGenerator, GenerateDefinitionRequest, GeneratedCommandDefinition,
    GeneratedSubcommandReport, GeneratedSummaryOutcome, GeneratedVariantDefinition, TraceRequest,
    TraceRunner,
};
pub use planner::{
    preferred_case_generator_from_env, CaseGenerationRequest, CaseGenerator, CaseGeneratorBackend,
    PlannerCaseGenerator,
};
