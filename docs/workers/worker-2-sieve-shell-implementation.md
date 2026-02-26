# Worker 2 Implementation: `sieve-shell`

Scope owned: `crates/sieve-shell` only.

## Implemented

- Added concrete analyzer: `BasicShellAnalyzer` implementing `ShellAnalyzer`.
- Implemented `analyze_shell_lc_script`.
- Supports composition operators only: `;`, `&&`, `||`, `|`.
- Deterministic segment extraction into ordered `CommandSegment` values.
- `operator_before` set per segment boundary.
- Knowledge classification:
  - `known`: fully parsed in MVP subset, segmentable argv.
  - `unknown`: syntax accepted but no summary-ready segment structure.
  - `uncertain`: unsupported construct detected.
- `unsupported_constructs` emitted for audit/debug.
- Added tests for:
  - valid composed commands,
  - unsupported constructs -> `uncertain`,
  - malformed parse -> `ShellAnalysisError::Parse`,
  - supported-but-empty -> `unknown`.

## Surprises

- `tree-sitter` path initially planned (matching referenced codex parser) but blocked by this workspace/network/dependency constraints during first pass.
- Implemented zero-new-dependency parser in `sieve-shell` to keep scope unblocked and testable.
- Full workspace test later succeeded once dependency/network path available.

## TODO / Follow-ups

- Optional hardening: switch to `tree-sitter-bash` for richer AST-based unsupported-construct detection parity with codex parser.
- Add more uncertain cases coverage: heredoc, command substitution forms, control-flow keywords.
- Add golden tests for edge quoting/escaping interactions across composition boundaries.
