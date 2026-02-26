# Worker 4 Implementation Notes: `sieve-policy`

## Scope Delivered

- Implemented `PolicyEngine::evaluate_precheck` in `crates/sieve-policy`.
- Added TOML policy loader + config model.
- Enforced:
  - deny rules (argv prefix matching),
  - unknown mode (`ask|accept|deny`),
  - uncertain mode (`ask|accept|deny`),
  - integrity gate for consequential actions,
  - required capability checks,
  - sink confidentiality checks (`value_ref` -> sink allow set).
- Added URL sink canonicalization helper aligned to MVP pin:
  - `url = 2.5.8`,
  - lowercase scheme/host,
  - default port elision (`:80`, `:443`),
  - query/fragment dropped,
  - dot-segment normalized by parser,
  - percent-decoding only for unreserved chars, uppercase hex for retained escapes.
- Added policy crate README with TOML format.

## Tests Added

- `rm -rf` blocked (`deny_with_approval`).
- POST missing capability denied.
- Payload sink violation denied.
- Mutating command denied when runtime control integrity is untrusted.
- Sink flow allowed when runtime sink permissions allow the value->sink pair.
- Sink flow still allowed from TOML `[value_sinks]` (compatibility fallback).
- Unknown/uncertain mode behavior matrix.
- URL canonicalization vectors.
- Composed-command all-or-nothing behavior.
- `violation_mode = ask` returns `deny_with_approval`.

## Surprises

- Workspace-wide `cargo test -p sieve-policy` became flaky after upstream/index resolution shift (`futures-sink ^0.3.32` visibility issue via unrelated crate graph).
- Isolated workspace verification (`sieve-types` + `sieve-policy`) passed cleanly.
- `url` produced lowercase hex in escapes; explicit normalization added to match spec language.

## Runtime Context Follow-Up (Chunk G)

- Consequential-action integrity gate now consumes `PrecheckInput.runtime_context.control.integrity`.
- Sink confidentiality checks now consume `PrecheckInput.runtime_context.sink_permissions`.
- TOML semantics preserved:
  - `[value_sinks]` still honored,
  - `options.trusted_control` remains a static top-level gate (runtime trust must also pass).

## Remaining TODOs

- Optional: add broader canonicalization vector corpus from spec once shared fixture location exists.
- Optional: add integration tests once full runtime loop chunk lands.
