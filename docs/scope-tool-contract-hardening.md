# Scope: Tool Contract Hardening (Planner Output + Runtime Validation)

Date: 2026-02-26
Owner lanes: `crates/sieve-types`, `crates/sieve-llm`, `crates/sieve-runtime`
Related question: what needs fixing in tool contracts.

## Objective

Move from permissive planner tool-call argument objects to strict, typed per-tool contracts with runtime validation, without changing MVP architecture decisions.

## Decisions (Locked)

- Tool contracts are authored as **Rust types** (single source of truth).
- Generate JSON schema artifacts from those Rust types for LLM `response_format` and documentation.
- On invalid tool-call args, allow one regeneration pass with compiler-like actionable diagnostics.
- Contract validation failures are logged internally in MVP and are **not** user-visible events.

## Why This Matters

Current behavior:

- Planner output schema allows:
  - `tool_name: string`
  - `args: object` with unrestricted keys/types.
- Runtime currently validates allowed tool names but not deep per-tool argument shape.

This is functional but permissive. Hardening reduces ambiguity, improves determinism, and narrows malformed tool-call surface.

## Proposed Work

1. Define a compile-time tool registry:
   - tool name
   - Rust typed args contract for each tool
   - generated JSON schema from Rust types
   - optional semantic validators.
2. Tighten planner output decode path:
   - validate each tool call args against registry schema
   - reject unknown keys/type mismatches early.
3. Add runtime guardrails:
   - verify decoded tool calls again before execution dispatch
   - emit structured internal log records on contract violation.
4. Version tool contracts:
   - registry version in runtime metadata/events for traceability.
5. Add contract tests:
   - positive examples per tool
   - negative fuzz/shape tests.
6. Add regeneration error channel:
   - for first validation failure, return a planner-visible structured diagnostic:
     - tool call index (`tool_calls[i]`)
     - line/column/range when recoverable from JSON parse
     - expected vs found type/field
     - actionable fix hint.
   - allow one retry generation pass, then hard fail.

## Deliverables

- Tool contract registry module.
- Validator integration in `sieve-llm` decode path.
- Validator integration in `sieve-runtime` pre-dispatch path.
- Contract test suite and documentation.
- Generated schema artifacts from Rust types committed under `schemas/`.

## Non-Goals

- Changing policy semantics.
- Adding new tool classes beyond MVP needs.
- Provider-specific planner behavior tuning beyond schema compliance.

## Risks

- Overly strict schemas can reduce model utility until prompts are tuned.
- Dual validation points can drift if not centralized.
