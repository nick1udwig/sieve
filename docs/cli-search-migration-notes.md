# CLI Search Migration Notes

## Decision
`brave_search` as a dedicated planner tool is being deprecated in app-facing flows.
Search should run through normal `bash`/CLI commands, aligned with the project philosophy that world interaction is commandline-driven.

## What Is Already Done
- `sieve-app` default `SIEVE_ALLOWED_TOOLS` no longer includes `brave_search`.
- `parse_allowed_tools` now strips `brave_search` even when provided in env.
- `.env.example` and README updated to describe bash/CLI-based search.

## Remaining Migration Work
1. Remove or gate remaining `brave_search`-specific app/runtime tests.
2. Update live app smoke tests from explicit `brave_search` prompts to CLI-based search prompts.
3. Decide whether to fully remove `brave_search` contracts/types/runtime path, or keep as dead-code compatibility layer for now.
4. If fully removed, follow through in:
   - `sieve-tool-contracts`
   - `sieve-runtime`
   - `sieve-types`
   - telegram/runtime test fixtures

## Policy Implication
CLI search commands still require explicit net-connect capability policy for target endpoints (for Brave API, `SIEVE_BRAVE_API_BASE`).
