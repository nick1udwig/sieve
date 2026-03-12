# Sieve v3 MVP Security Design

## Decision: execpolicy
Do not include `execpolicy` in MVP.

Reason:
- `execpolicy` is useful for coarse command prefix allow/prompt/deny.
- MVP requires per-argument and per-data-value enforcement (integrity + confidentiality at sinks).
- Extending `execpolicy` to cover this would effectively become a different policy system.

Use a dedicated `cap-policy` format + small Rust evaluator for MVP.

## MVP goals
- Enforce both integrity and confidentiality.
- Enforce per-argument policy, not only per-command policy.
- Support trusted single-user assistant model.

## Explicit non-goals for MVP
- Side-channel resistance (post-MVP TODO).
- Persistent command learning/profile DB (post-MVP TODO).

## Core model

### Capability effects
`cap = (resource, action, scope)`

- resource: `fs | net | proc | env | ipc`
- action: `read | write | append | exec | connect`
- scope examples:
  - fs: absolute path or path pattern
  - net: scheme + host[:port] pattern
  - proc: binary path/name

### Data labels
Each value carries a label:
- provenance: source chain (`user`, `tool:<name>`, `quarantine:<cmd>`, etc.)
- integrity: `trusted | untrusted`
- allowed_sinks: set of `(sink, channel)` permissions this value may flow to
- capacity_type: `bool | int | float | enum | trusted_string`

This allows confidentiality checks at sinks and integrity checks on control decisions.
Declassification should mint a derived release value rather than mutating the source label in place.
Runtime may separately track source-to-release grants so real sink checks can honor the approved release without widening the source label.

## Policy checks

### Integrity check (control)
Consequential actions must not be triggered by untrusted control context unless explicitly endorsed.

### Confidentiality check (data flow)
For every sink argument, verify each value flowing into that sink is allowed for that sink and channel.

### Capacity-aware explicit release
`trusted_string` values must not be endorsed to trusted control or declassified directly.
Release and endorsement should operate on bounded typed extracts instead.
Approved `declassify` should return a release `value_ref` scoped to one sink and channel.
Runtime policy should also deny sink flow for runtime-labeled `trusted_string` values and refuse to treat them as trusted control.

## Per-argument enforcement examples

1. Never allow:
- `rm -rf ...`

2. Conditional allow:
- `curl -X POST URL ...` allowed only if command has `net.write(URL)`.

3. Payload flow constraint:
- `curl -X POST URL -d BODY` requires BODY label to permit sink `net:URL`.

## Declarative format (MVP sketch)
Use a small TOML format, e.g.:

```toml
[[command_rule]]
id = "deny-rm-rf"
match = ["rm", "-rf"]
decision = "deny"

[[command_summary]]
id = "curl"
match = ["curl"]

[command_summary.args]
method_flag = "-X"
url = { kind = "first_url" }
body = { flags = ["-d", "--data", "--data-binary"] }

[[command_summary.required_caps]]
when = { method = "GET" }
cap = "net.read:${url}"

[[command_summary.required_caps]]
when = { method = "POST" }
cap = "net.write:${url}"

[[command_summary.sink_checks]]
arg = "body"
sink = "net:${url}"
mode = "all_values_must_allow"
```

## Execution flow (MVP)
1. Parse command/shell AST and derive candidate effects from command summaries.
2. Evaluate policy (integrity + confidentiality + required effect caps).
3. If known command and policy passes, execute.
4. If unknown command, run quarantine trace path (below).

## Unknown command handling (MVP)
Default unknown behavior: quarantine-run with trace and user-visible report.

- Run command in strong quarantine:
  - Linux bwrap sandbox
  - no network
  - read-only FS except minimal temp scratch
- Trace with `strace -ff`.
- Capture attempted effects (`execve`, file writes, network connect, etc.).
- Present attempted capability footprint to user for explicit approval.
- Do not persist learned profile in MVP.

## Runtime interception option
Use patched Bash from `https://github.com/bolinfest/bash` with `EXEC_WRAPPER`/`BASH_EXEC_WRAPPER` to intercept child `execve` and re-check policy at runtime.

This is optional for MVP but strongly recommended because it improves visibility for composed shell behavior and subprocesses.

## Suggested MVP components
- `cap-policy` crate: parse declarative policy + summaries.
- `cap-infer` crate: map parsed commands to required effects and sink arguments.
- `cap-enforcer` crate: perform integrity/confidentiality checks.
- `quarantine-runner` crate: bwrap + strace execution and normalized trace report.

## Acceptance criteria for MVP
- Blocks `rm -rf` unconditionally.
- Blocks `curl -X POST URL -d BODY` if `net.write(URL)` missing.
- Blocks POST if BODY cannot flow to sink `URL`.
- Unknown commands always execute only in quarantine mode and emit a trace summary for user approval.
