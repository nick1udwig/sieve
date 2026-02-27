# sieve-captrace

Standalone capability-definition generator for one CLI command.

Flow:
- collect argument variants (seed cases + optional planner LLM)
- run each variant through quarantine tracing (`bwrap + strace`)
- emit Sieve-compatible `summary_outcome.summary` definitions per variant

## Usage

```bash
cargo run -p sieve-captrace -- mkdir --seed-case 'mkdir -p {{TMP_DIR}}/logs' --output /tmp/mkdir-definition.json
```

### Placeholders

- `{{TMP_DIR}}`
- `{{IN_FILE}}`
- `{{IN_FILE_2}}`
- `{{OUT_FILE}}`

## LLM Mode

Enabled by default (disable with `--no-llm`).

Required env:
- `SIEVE_PLANNER_MODEL`
- `OPENAI_API_KEY` (or `SIEVE_PLANNER_OPENAI_API_KEY`)

Optional:
- `SIEVE_PLANNER_API_BASE`
