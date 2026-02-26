# sieve-policy

TOML policy evaluator for pre-exec checks.

## Format

```toml
[[deny_rules]]
id = "deny-rm-rf"
argv_prefix = ["rm", "-rf"]
decision = "deny_with_approval" # deny | deny_with_approval
reason = "rm -rf requires approval"

[[allow_capabilities]]
resource = "net" # fs | net | proc | env | ipc
action = "write" # read | write | append | exec | connect
scope = "https://api.example.com/v1/upload"

[value_sinks]
body_ref = ["https://api.example.com/v1/upload"]

[options]
violation_mode = "deny" # deny | ask
require_trusted_control_for_mutating = true
trusted_control = true
```

Notes:
- Net scopes and sink URLs canonicalized with `url = 2.5.8` behavior.
- Query/fragment ignored for sink keys.
- Unknown/uncertain handling is from `PrecheckInput` modes (`ask|accept|deny`).
