# Fides-Inspired Sink Hardening

## Goal

Make sink authorization narrower and harder to misuse.
Use Fides ideas where they fit the current sieve architecture.
Land concrete hardening now.
Keep larger planner-memory work staged.

## Current Gaps

Sinks are authorized mostly as `value_ref -> URL`.
That is too coarse for multi-channel egress.
Approving body flow to a sink can imply too much if headers or query are not separated.
Explicit tools are evaluated without threaded control dependencies.
Runtime label propagation is still partial.

## This Branch

### Phase 1

Add channel-scoped sink permissions.
Make `declassify` approve `value_ref -> sink -> channel`, not only `value_ref -> sink`.
Teach sink checks to compare both sink and channel.
Cover `curl` headers as explicit sink-bearing egress.
Thread planner control context into explicit tool policy evaluation.
Thread opaque untrusted memory refs from planner feedback into the next-step control context.
Status: done.

### Phase 2

Use `capacity_type` in policy decisions.
Prefer bounded-capacity releases over free-text release.
Add stronger rules for explicit tools under untrusted control.
Status: mostly done.
Shipped here: `trusted_string` values cannot be endorsed to `trusted` and cannot be declassified.
Shipped here: explicit-tool requests for unknown `value_ref` fail before approval.
Shipped here: policy denies sink flow for runtime-labeled `trusted_string` values even if ambient sink grants exist.
Shipped here: `trusted_string` values do not count as trusted control context.
Shipped here: planner hides `endorse`/`declassify` unless eligible bounded refs actually exist.

### Phase 3

Move from coarse declassification to derived release values.
Hide raw values from planner and reveal typed extracts only.
This is the closest Fides analogue.
It likely needs planner-memory work and a quarantined extractor path.
Status: partial.
Shipped here: `declassify` mints a derived release `value_ref`.
Shipped here: source labels stay unchanged; runtime policy tracks source-to-release sink grants separately.

## Acceptance For Phase 1

Body approval does not authorize header exfil to the same URL.
`declassify` requires a channel and records it in runtime state.
Policy engine denies channel mismatch.
Explicit tools honor threaded control context from runtime callers.
Tests cover policy, runtime, and command-summary behavior.

## Remaining Gap

Typed extraction is still missing.
Current `declassify` derives a release ref from an already-labeled bounded value.
That is narrower than source-label mutation, but still short of a full Fides-style quarantined reveal primitive.
