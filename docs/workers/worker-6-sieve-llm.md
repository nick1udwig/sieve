# Worker 6: `sieve-llm`

You are Worker 6 for Sieve v3. Own only crate `crates/sieve-llm`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md` (planner/quarantine split and typed q->p)
- `/root/git/sieve-v3/docs/agent-prompt-constraints-v1.2.md`
- `/root/git/sieve-v3/crates/sieve-llm/src/lib.rs`
- `/root/git/camel-prompt-injection/src/camel/pipeline_elements/privileged_llm.py`
- `/root/git/camel-prompt-injection/src/camel/quarantined_llm.py`

## Mission

Implement real OpenAI-backed planner and quarantined extract model adapters.

## Scope

- Implement concrete `PlannerModel` and `QuarantineModel`.
- OpenAI-only in MVP, but keep provider abstraction and independent P/Q configs.
- Q model must output typed values only: bool, int, float, enum.
- No deterministic stubs.
- Enforce "no untrusted string to planner" boundary at API layer.
- Handle retry and errors cleanly.

## Required Outputs

- OpenAI client integration and config loading.
- Structured output handling for typed extraction.
- Tests or mocks for serialization and typed decoding paths.

## Out Of Scope

- Runtime orchestration.
- Policy decisions.

## Definition Of Done

- `cargo test -p sieve-llm` passes.
- Demonstrate one real integration call path (env-gated test or example).
