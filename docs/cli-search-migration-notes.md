# CLI Search Migration Notes

## Decision
`brave_search` as a dedicated planner tool is removed.
Search runs through normal `bash`/CLI commands, aligned with the project philosophy that world interaction is commandline-driven.

## Completed Migration
- `sieve-app` default `SIEVE_ALLOWED_TOOLS` no longer includes `brave_search`.
- `parse_allowed_tools` now strips `brave_search` even when provided in env.
- `.env.example` and README updated to describe bash/CLI-based search.
- `sieve-tool-contracts`, `sieve-runtime`, and `sieve-types` no longer define or dispatch `brave_search`.
- Runtime/app/telegram test fixtures no longer require web-search runner stubs.
- Live app smoke tests no longer include dedicated `brave_search` flow coverage.

## Policy Implication
CLI search commands still require explicit net-connect capability policy for target endpoints (for Brave API, `SIEVE_BRAVE_API_BASE`).
