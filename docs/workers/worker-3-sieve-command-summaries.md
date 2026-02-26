# Worker 3: `sieve-command-summaries`

You are Worker 3 for Sieve v3. Own only crate `crates/sieve-command-summaries`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md` (command summaries and codex snapshot rule)
- `/root/git/sieve-v3/crates/sieve-command-summaries/src/lib.rs`
- `/root/git/codex/codex-rs/shell-command/src/command_safety/is_safe_command.rs`
- `/root/git/codex/codex-rs/shell-command/src/command_safety/is_dangerous_command.rs`
- Codex commit pinned by spec: `79d6f80`

## Mission

Implement per-command summary inference (required capabilities plus sink checks), seeded from Codex command classes via git dependency.

## Scope

- Implement `CommandSummarizer::summarize`.
- Use codex snapshot logic as baseline classification.
- Map at least these MVP-critical cases:
  - `rm -rf` class.
  - `curl -X POST URL`.
  - `curl -X POST URL -d BODY`.
  - Safe read-ish commands needed for early integration.
- Unsupported and unknown flags route to unknown handling.
- Return structured `SummaryOutcome`.

## Required Outputs

- Git dependency wiring to codex snapshot commit.
- Unit tests for mapped commands and flag behavior.
- Explicit sink extraction for payload-bearing args.

## Out of Scope

- Final allow or deny decisions.
- Shell parsing.
- Quarantine execution.

## Definition Of Done

- `cargo test -p sieve-command-summaries` passes.
- Tests demonstrate required capabilities and sinks for `curl` POST cases.
