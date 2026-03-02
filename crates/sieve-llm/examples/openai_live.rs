// For the integrated runtime + Telegram app entrypoint, run `cargo run -p sieve-app -- "<prompt>"`.
use sieve_llm::{GuidanceModel, OpenAiGuidanceModel};
use sieve_types::{LlmModelConfig, LlmProvider, PlannerGuidanceInput, RunId};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key =
        env::var("OPENAI_API_KEY").map_err(|_| "missing OPENAI_API_KEY for live example")?;
    let model = env::var("SIEVE_GUIDANCE_MODEL")
        .or_else(|_| env::var("SIEVE_QUARANTINE_MODEL"))
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let api_base = env::var("SIEVE_GUIDANCE_API_BASE")
        .ok()
        .or_else(|| env::var("SIEVE_QUARANTINE_API_BASE").ok());

    let guidance_model = OpenAiGuidanceModel::new(
        LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model,
            api_base,
        },
        api_key,
    )?;

    let input = PlannerGuidanceInput {
        run_id: RunId("example-live".to_string()),
        prompt: "User asked for a direct answer and no tool evidence is needed.".to_string(),
    };
    let output = guidance_model.classify_guidance(input).await?;
    println!("{:?}", output.guidance);
    Ok(())
}
