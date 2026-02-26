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
5. Q -> P types: `bool | int | float | enum` only.
6. Untrusted strings never reach Planner.
7. Trusted strings allowed only from explicitly trusted sources.
8. Enums are compile-time Rust registry only (no runtime enum definition).
9. Both integrity and confidentiality checks are mandatory.
10. Consequential-action integrity checks apply only to mutating/unknown commands.
11. Mutating/safe classification is copied from Codex allowlist/denylist snapshot at commit `79d6f80`.
12. `rm -rf` class is `deny_with_approval`.
13. Default policy-violation behavior is deny; configurable to ask.
14. Policy language is pure TOML (no expression language).
15. URL sink matching uses canonical `scheme://host[:port]/path`; ignore query/fragment.
16. URL canonicalization includes percent-encoding and dot-segment normalization.
17. URL canonicalization implementation is pinned to Rust crate `url = 2.5.8`.
18. No globs in MVP (future work).
19. No alias-in-alias expansion.
20. Unsupported/unknown flags in summaries route as unknown command handling.
21. Supported Bash composition operators: `;`, `&&`, `||`, `|`.
22. No heredoc in MVP.
23. No redirections in MVP.
24. Unsupported shell constructs are `uncertain`.
25. `uncertain` default is hard deny; configurable similarly to unknown mode.
26. `unknown_command_mode` default is `deny`.
27. Unknown mode options: `ask | accept | deny`.
28. Unknown/uncertain execution policy is all-or-nothing precheck for composed commands.
29. Quarantine execution: Linux `bwrap` + no network + minimal writable scratch + `strace -ff`.
30. Pre-exec enforcement only in MVP.
31. Unknown/accepted quarantine runs log trace and notify user where logs are stored.
32. If easy, include stdout/stderr in quarantine logs; else omit in MVP.
33. Logs root: `~/.sieve/logs/`.
34. Logs are append-only and retained forever by default in MVP.
35. Retention/redaction are future work.
36. `endorse` and `declassify` are explicit tools.

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
- `allowed_sinks`: concrete sink set
- `capacity_type`: `bool | int | float | enum | trusted_string`

Rules:
- Non-trusted strings cannot flow to Planner.
- Typed Q outputs may flow to Planner, but remain policy-controlled for consequential use.

## 4. Enforcement semantics

### 4.1 Integrity (P-T style)
For mutating/unknown commands:
- require trusted control context OR
- require explicit approval/endorsement.

### 4.2 Confidentiality (P-F style)
For each sink argument:
- all payload values flowing to sink must allow that exact sink.

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
    "value_ref": "v123e",
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
- relax confidentiality for a specific sink.

Request:
```json
{
  "tool": "declassify",
  "params": {
    "value_ref": "v456",
    "sink": "https://api.example.com/v1/upload",
    "reason": "optional-human-readable"
  }
}
```

Response:
```json
{
  "ok": true,
  "result": {
    "value_ref": "v456d",
    "allowed_sinks_added": ["https://api.example.com:443/v1/upload"]
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
- Approvals are one-shot only in MVP.
- No automatic endorse/declassify.

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
- save trace artifacts under `~/.sieve/logs/traces/<run_id>/`.
- notify user with location.
- no policy-learning side effects in MVP.

Audit events (JSONL) under `~/.sieve/logs/events/` include:
- timestamp, run_id
- command/cwd
- inferred caps
- policy decision + reason
- approval outcome
- trace path
- stdout/stderr metadata if captured

## 11. Agent prompt constraints
Use companion file:
- `agent-prompt-constraints-v1.2.md`
