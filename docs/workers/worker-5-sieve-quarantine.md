# Worker 5: `sieve-quarantine`

You are Worker 5 for Sieve v3. Own only crate `crates/sieve-quarantine`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md` (quarantine and logs)
- `/root/git/sieve-v3/docs/sieve-v3-mvp-security.md` (quarantine execution)
- `/root/git/sieve-v3/crates/sieve-quarantine/src/lib.rs`
- `/root/git/codex/codex-rs/linux-sandbox/src/bwrap.rs`
- `/root/git/codex/codex-rs/linux-sandbox/src/linux_run_main.rs`

## Mission

Implement quarantine execution (`bwrap` plus no-net plus `strace -ff`) and normalized reporting.

## Scope

- Implement `QuarantineRunner::run`.
- Store artifacts under `~/.sieve/logs/traces/<run_id>/`.
- Capture trace file locations and attempted capabilities.
- Capture stdout and stderr metadata and paths if straightforward.
- Return `QuarantineReport` for runtime events.

## Required Outputs

- Robust process execution and error handling.
- Parser for trace lines into attempted capability list.
- Tests for path layout and report format.

## Out of Scope

- Policy decision logic.
- LLM integration.

## Definition Of Done

- `cargo test -p sieve-quarantine` passes.
- Manual smoke test demonstrates trace artifact generation.
