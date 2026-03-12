# Sieve v3 MVP Spec v1.3

## 1. Scope
Sieve v3 MVP is an always-on personal assistant with:
- One user.
- World interaction exclusively via Bash commands.
- Planner/Quarantine split inspired by FIDES/CaMeL.
- Capability-based enforcement with both integrity and confidentiality.

Not in MVP:
- Formal proofs.
- Side-channel resistance.
- Auto-learning that changes future policy behavior.

## 2. Locked decisions
1. Bash-only world interaction.
2. Planner/Quarantine split required.
3. Default trusted roots: user intent/prompt + local config.
4. Local files are default untrusted (configurable).
5. Q -> P channel uses typed planner-guidance signals only (Rust enum, numeric wire frame).
6. Untrusted strings never reach Planner.
7. Trusted strings allowed only from explicitly trusted sources.
8. Planner-guidance signal variants are compile-time Rust enum variants (no runtime variant definition).
9. Planner-guidance enum is intentionally extensible; adding many variants over time is expected.
10. Both integrity and confidentiality checks are mandatory.
11. Consequential-action integrity checks apply only to mutating/unknown commands.
12. Mutating/safe classification is copied from Codex allowlist/denylist snapshot at commit `79d6f80`.
13. `trash` class is `deny_with_approval`; raw `rm` stays dangerous and unknown.
14. Default policy-violation behavior is deny; configurable to ask.
15. Policy language is pure TOML (no expression language).
16. URL sink matching uses canonical `scheme://host[:port]/path`; ignore query/fragment.
17. URL canonicalization includes percent-encoding and dot-segment normalization.
18. URL canonicalization implementation is pinned to Rust crate `url = 2.5.8`.
19. No globs in MVP (future work).
20. No alias-in-alias expansion.
21. Unsupported/unknown flags in summaries route as unknown command handling.
22. Supported Bash composition operators: `;`, `&&`, `||`, `|`.
23. No heredoc in MVP.
24. No redirections in MVP.
25. Unsupported shell constructs are `uncertain`.
26. `uncertain` default is hard deny; configurable similarly to unknown mode.
27. `unknown_command_mode` default is `deny`.
28. Unknown mode options: `ask | accept | deny`.
29. Unknown/uncertain execution policy is all-or-nothing precheck for composed commands.
30. Quarantine execution: Linux `bwrap` + no network + minimal writable scratch + `strace -ff`.
31. Pre-exec enforcement only in MVP.
32. Unknown/accepted quarantine runs log trace and notify user where logs are stored.
33. If easy, include stdout/stderr in quarantine logs; else omit in MVP.
34. Logs root: `~/.sieve/logs/`.
35. Logs are append-only and retained forever by default in MVP.
36. Retention/redaction are future work.
37. `endorse` and `declassify` are explicit tools.
38. Q-LLM guidance classification is integrated in the planner act-observe loop for MVP turns.
39. Compose quality can request additional planner/tool cycles only through typed guidance signals (no free-form diagnostics to planner).

## 3. Data model

### 3.1 Capability tuple
`cap = (resource, action, scope)`

- resource: `fs | net | proc | env | ipc`
- action: `read | write | append | exec | connect`
- scope:
  - fs: canonical absolute path
  - net: canonical URL (query/fragment excluded)
  - proc: executable name/path

### 3.2 Labels per runtime value
- `integrity`: `trusted | untrusted`
- `provenance`: source lineage
- `allowed_sinks`: concrete `(sink, channel)` set
- `capacity_type`: `bool | int | float | enum | trusted_string`

Rules:
- Non-trusted strings cannot flow to Planner.
- Typed Q guidance signals may flow to Planner, but remain policy-controlled for consequential use.
- `trusted_string` values must not be endorsed to `trusted` and must not be declassified directly.
- `declassify` must mint a derived release value for sink use; do not mutate the source label in place.
- Runtime policy must deny sink flow for runtime-labeled `trusted_string` values.
- Runtime policy must treat `trusted_string` control refs as untrusted for consequential actions.
- Planner should expose `endorse`/`declassify` only when at least one eligible bounded value exists for that action.

## 4. Enforcement semantics

### 4.1 Integrity (P-T style)
For mutating/unknown commands:
- require trusted control context OR
- require explicit approval/endorsement.

### 4.2 Confidentiality (P-F style)
For each sink argument:
- all payload values flowing to sink must allow that exact sink and channel.

### 4.3 Per-argument examples
- `rm -rf X` -> `deny_with_approval`.
- `curl -X POST URL` -> require `net.write(URL)`.
- `curl -X POST URL -d BODY` -> BODY must allow sink URL.

### 4.4 Composed command precheck
For commands composed with `; && || |`:
- infer/check all component actions before execution.
- if any component denies under active mode, execute none.

## 5. Unknown and uncertain handling
- `unknown`: syntax supported, but no summary (or unsupported flags route here).
- `uncertain`: parser/semantic extraction outside supported subset.

Modes:
- `deny` (default)
- `ask`
- `accept` (quarantine-run)

## 6. Command summaries
Each summary defines:
- match signature
- arg extraction
- required capabilities
- sink checks
- unsupported flag behavior -> unknown

Bootstrap source for MVP command classes:
- Codex snapshot `79d6f80` allowlist/denylist + parsing behaviors.

## 7. Explicit tools: endorse/declassify

### 7.1 Tool contract (JSON schema style)

#### Tool: `endorse`
Purpose:
- upgrade integrity of typed value for control-use.
- `trusted_string` is not eligible; derive a bounded typed value first.

Request:
```json
{
  "tool": "endorse",
  "params": {
    "value_ref": "v123",
    "target_integrity": "trusted",
    "reason": "optional-human-readable"
  }
}
```

Response:
```json
{
  "ok": true,
  "result": {
    "value_ref": "v123",
    "integrity": "trusted"
  }
}
```

Failure:
```json
{
  "ok": false,
  "error": {
    "code": "approval_required|policy_denied|invalid_value_ref",
    "message": "..."
  }
}
```

#### Tool: `declassify`
Purpose:
- relax confidentiality for a specific sink and channel.
- `trusted_string` is not eligible; derive a bounded typed value first.
- success mints a derived release `value_ref` scoped to that sink and channel.

Request:
```json
{
  "tool": "declassify",
  "params": {
    "value_ref": "v456",
    "sink": "https://api.example.com/v1/upload",
    "channel": "body",
    "reason": "optional-human-readable"
  }
}
```

Response:
```json
{
  "ok": true,
  "result": {
    "value_ref": "v456",
    "release_value_ref": "vrel_1",
    "allowed_sinks_added": [
      {
        "sink": "https://api.example.com:443/v1/upload",
        "channel": "body"
      }
    ]
  }
}
```

Failure:
```json
{
  "ok": false,
  "error": {
    "code": "approval_required|policy_denied|invalid_sink|invalid_value_ref",
    "message": "..."
  }
}
```

### 7.2 Approval and execution semantics for explicit tools
- Both tools require policy check and user approval.
- Both tools fail before approval if `value_ref` is unknown.
- Approvals are one-shot only in MVP.
- No automatic endorse/declassify.
- `declassify` leaves the source label unchanged and creates a derived release `value_ref`.
- Runtime policy may honor that release for later use of the same source value only at the approved sink and channel.

## 8. deny_with_approval semantics (proposed and adopted)

For a command/action resolved as `deny_with_approval`:
1. Execution is blocked pre-exec.
2. User is shown:
   - full command argv
   - inferred capabilities
   - blocked rule ID and reason
3. User choices:
   - `approve_once`
   - `deny`
4. On `approve_once`, that single attempted command proceeds (subject to all-or-nothing composed precheck).
5. No persistent policy/allowlist mutation from this approval in MVP.

## 9. URL canonicalization implementation (pinned)
Use `url = 2.5.8` and canonicalization procedure:
1. Parse via `url::Url`.
2. Lowercase scheme and host.
3. Apply default port elision (`:80` for http, `:443` for https).
4. Normalize path dot-segments.
5. Normalize percent-encoding to canonical uppercase hex triplets and decode only unreserved characters.
6. Drop query and fragment for sink key.
7. Result sink key format: `scheme://host[:port]/path`.

Implementation must ship with deterministic test vectors.

## 10. Quarantine execution and logs
Unknown/accepted path:
- execute in quarantine (`bwrap`, no-net, scratch write, `strace -ff`).
- save trace artifacts under `~/.sieve/logs/traces/<turn_id>/`.
- notify user with location.
- feed only typed numeric Q-LLM guidance frames into planner/runtime loop (never free-form strings).
- no policy-learning side effects in MVP.

Canonical event stream (JSONL) under `~/.sieve/logs/runtime-events.jsonl` includes:
- timestamp, session_id, turn_id, turn_seq
- ingress/conversation events
- policy decision + reason
- approval outcome
- planner guidance and compose follow-up decisions
- trace path and artifact refs when captured

## 11. Agent prompt constraints
Use companion file:
- `agent-prompt-constraints-v1.2.md`
