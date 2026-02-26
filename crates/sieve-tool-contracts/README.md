# sieve-tool-contracts

Strict typed tool argument contracts for Sieve MVP tools.

## Scope

- Rust types are the source of truth for tool args.
- Validator API decodes `tool_name + args` into typed calls.
- JSON schema artifacts are emitted from the same Rust types.

## Supported tools

- `bash`
- `endorse`
- `declassify`

## API

```rust
use serde_json::json;
use sieve_tool_contracts::{validate, TypedCall};

let typed = validate("bash", &json!({"cmd": "ls -la"}))?;
match typed {
    TypedCall::Bash(args) => assert_eq!(args.cmd, "ls -la"),
    _ => unreachable!(),
}
```

## Emit schema artifacts

```bash
cargo run --manifest-path crates/sieve-tool-contracts/Cargo.toml --bin emit-schemas
```

Outputs are written to `crates/sieve-tool-contracts/schemas/`.
