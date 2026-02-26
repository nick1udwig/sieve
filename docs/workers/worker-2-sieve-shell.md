# Worker 2: `sieve-shell`

You are Worker 2 for Sieve v3. Own only crate `crates/sieve-shell`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md` (Shell subset and unknown/uncertain sections)
- `/root/git/sieve-v3/crates/sieve-shell/src/lib.rs`
- `/root/git/codex/codex-rs/shell-command/src/bash.rs`
- `/root/git/codex/codex-rs/shell-command/src/parse_command.rs`

## Mission

Implement shell analysis for the MVP subset and produce `ShellAnalysis`.

## Scope

- Implement `ShellAnalyzer::analyze_shell_lc_script`.
- Support only composition operators: `;`, `&&`, `||`, `|`.
- Extract ordered `CommandSegment`s.
- Classify input as:
  - `known` when fully in supported subset.
  - `unknown` when syntax is supported but no summary-ready structure.
  - `uncertain` for unsupported constructs (heredoc, redirects, substitutions, control flow, etc.).
- Emit `unsupported_constructs` details for audit and debug.

## Required Outputs

- Parser implementation plus tests:
  - Valid composed commands.
  - Unsupported constructs map to uncertain.
  - Malformed parse maps to error.
- Deterministic segmentation order.

## Out of Scope

- Capability inference.
- Policy decisions.
- Quarantine execution.

## Definition Of Done

- `cargo test -p sieve-shell` passes.
- Output shape matches `sieve-types` contracts.
