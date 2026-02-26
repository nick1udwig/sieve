# Worker 4: `sieve-policy`

You are Worker 4 for Sieve v3. Own only crate `crates/sieve-policy`.

## Read First

- `/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md`
- `/root/git/sieve-v3/docs/sieve-v3-mvp-security.md`
- `/root/git/sieve-v3/crates/sieve-policy/src/lib.rs`
- URL canonicalization section in MVP spec (must pin `url = 2.5.8` semantics)

## Mission

Implement the policy evaluator for pre-exec decisions.

## Scope

- Implement `PolicyEngine::evaluate_precheck`.
- Enforce integrity and confidentiality checks from contracts.
- Implement unknown and uncertain modes `ask|accept|deny`.
- Enforce all-or-nothing precheck for composed commands.
- Return `PolicyDecision` with reason and blocked rule ID where applicable.
- Add TOML policy parsing (no expression language).

## Required Outputs

- URL sink canonicalization aligned with spec.
- Deterministic decision behavior.
- Tests for:
  - `rm -rf` blocked path.
  - POST missing cap denied.
  - Payload sink violation denied.
  - Unknown and uncertain mode behaviors.

## Out of Scope

- Approval transport.
- Quarantine runner internals.

## Definition Of Done

- `cargo test -p sieve-policy` passes.
- Policy behavior matches MVP examples.
