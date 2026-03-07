# sieve-policy

TOML policy evaluator for pre-exec checks.

## Format

```toml
[[deny_rules]]
id = "deny-trash"
argv_prefix = ["trash"]
decision = "deny_with_approval" # deny | deny_with_approval
reason = "trash requires approval"

[[allow_capabilities]]
resource = "net" # fs | net | proc | env | ipc
action = "write" # read | write | append | exec | connect
scope = "https://api.example.com/v1/upload"

[value_sinks]
body_ref = ["https://api.example.com/v1/upload"]

[options]
violation_mode = "deny" # deny | ask
require_trusted_control_for_mutating = true
trusted_control = true # optional static gate; runtime control context must also be trusted
```

Notes:
- Net scopes and sink URLs canonicalized with `url = 2.5.8` behavior.
- Query/fragment ignored for sink keys.
- Unknown/uncertain handling is from `PrecheckInput` modes (`ask|accept|deny`).
- Consequential-action integrity checks read `PrecheckInput.runtime_context.control.integrity`.
- Sink flow checks read `PrecheckInput.runtime_context.sink_permissions` and also honor TOML `[value_sinks]`.
