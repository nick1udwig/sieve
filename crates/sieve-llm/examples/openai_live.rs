// For the integrated runtime + Telegram app entrypoint, run `cargo run -p sieve-app -- "<prompt>"`.
use sieve_llm::{OpenAiQuarantineModel, QuarantineModel};
use sieve_types::{LlmModelConfig, LlmProvider, QuarantineExtractInput, RunId};
use std::collections::BTreeMap;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key =
        env::var("OPENAI_API_KEY").map_err(|_| "missing OPENAI_API_KEY for live example")?;
    let model = env::var("SIEVE_QUARANTINE_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let api_base = env::var("SIEVE_QUARANTINE_API_BASE").ok();

    let quarantine = OpenAiQuarantineModel::new(
        LlmModelConfig {
            provider: LlmProvider::OpenAi,
            model,
            api_base,
        },
        api_key,
    )?;

    let input = QuarantineExtractInput {
        run_id: RunId("example-live".to_string()),
        prompt: "Return int 7.".to_string(),
        enum_registry: BTreeMap::new(),
    };
    let output = quarantine.extract_typed(input).await?;
    println!("{:?}", output.value);
    Ok(())
}
