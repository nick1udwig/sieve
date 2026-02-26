use sieve_tool_contracts::emitted_schema_documents;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let output_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas");
    fs::create_dir_all(&output_dir)?;

    for (filename, schema) in emitted_schema_documents() {
        let path = output_dir.join(filename);
        let mut encoded = serde_json::to_string_pretty(&schema)?;
        encoded.push('\n');
        fs::write(path, encoded)?;
    }

    Ok(())
}
