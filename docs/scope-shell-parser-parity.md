# Scope: Shell Parser Parity With Codex

Date: 2026-02-26
Owner lane: `crates/sieve-shell`
Related question: "optional future upgrade" relative to Codex parser behavior.

## Objective

Upgrade `sieve-shell` from the current lightweight splitter/tokenizer to an AST-driven parser with behavior closer to Codex's `tree-sitter-bash` approach, while preserving Sieve MVP decisions:

- Supported composition operators: `;`, `&&`, `||`, `|`.
- Unsupported constructs classified as `uncertain`.
- Syntax-supported but non-summary-ready shapes classified as `unknown`.

## Decisions (Locked)

- Target **security-equivalent** behavior, not strict line-by-line Codex parity.
- Reuse Codex parser logic/components where they work cleanly for Sieve.
- Keep Sieve-specific behavior when needed for MVP constraints.

## Current State

- `BasicShellAnalyzer` uses string scanning/tokenization.
- It already classifies known/unknown/uncertain and extracts ordered segments.
- It does not fully mirror Codex parser edge behavior for quoting, escapes, and nested structures.

## Proposed Work

1. Introduce AST parsing via `tree-sitter-bash`.
2. Re-implement segment extraction from AST command nodes in source order.
3. Implement explicit unsupported-construct detection from AST node kinds:
   - heredoc
   - redirection
   - command substitution
   - process substitution
   - control-flow and grouping constructs
4. Define deterministic mapping rules:
   - supported + extractable -> `known`
   - supported parse but not extractable -> `unknown`
   - unsupported construct present -> `uncertain`
5. Add parity-oriented test corpus:
   - copied/derived from Codex shell command tests where applicable
   - Sieve-specific uncertain cases.
6. Keep existing public trait/contracts unchanged unless absolutely required.

## Deliverables

- Updated `sieve-shell` implementation using AST parser.
- Unit tests covering:
  - composition extraction
  - unsupported detection
  - known vs unknown vs uncertain boundaries.
- Short behavior matrix doc in crate README.

## Non-Goals

- Capability inference.
- Policy decisions.
- Runtime execution behavior.

## Risks

- Parser/AST dependency drift versus Codex snapshot behavior.
- Over-classifying to `uncertain` and reducing utility.

## Default Assumptions

- Newline separators remain conservative unless explicitly promoted:
  - treat as `uncertain` by default in MVP parser parity work.
- Add fixture corpus including imported Codex-like cases and Sieve-specific cases.
