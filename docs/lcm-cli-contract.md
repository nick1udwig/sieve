# Sieve LCM CLI Contract (v1)

This document defines the stable CLI boundary between `sieve-v3` and `sieve-lcm` for planner-driven memory retrieval.

## Goals

- Remove automatic memory text injection into planner prompts.
- Expose memory via command-line tools in the act/observe loop.
- Preserve trust model:
  - trusted lane data may be returned as plain text.
  - untrusted lane data must be returned as opaque refs (no raw strings to planner).

## Binary

- CLI binary: `sieve-lcm-cli`

## Global Conversation

- All operations default to one global conversation key: `global`.
- Override via `--conversation <id>` only for tests/debug.

## Commands

### `query`

Retrieve memory for a planner query.

```bash
sieve-lcm-cli query \
  --query "where do i live" \
  --limit 5 \
  --lane both \
  --json
```

Arguments:
- `--trusted-db <path>`: optional; defaults from `SIEVE_LCM_TRUSTED_DB_PATH` then `~/.sieve/lcm/trusted.db`.
- `--untrusted-db <path>`: optional; defaults from `SIEVE_LCM_UNTRUSTED_DB_PATH` then `~/.sieve/lcm/untrusted.db`.
- `--conversation <id>`: optional; defaults from `SIEVE_LCM_GLOBAL_SESSION_ID` then `global`.
- `--query <text>`: retrieval prompt.
- `--limit <n>`: max hits per lane (default `5`, min `1`, max `20`).
- `--lane <trusted|untrusted|both>`: defaults to `both`.
- `--json`: required for machine consumption.

Output JSON schema:

```json
{
  "conversation": "global",
  "query": "where do i live",
  "trusted_hits": [
    {
      "id": "trusted:msg:42",
      "source": "message",
      "score": 0.92,
      "created_at": "2026-03-05T00:00:00Z",
      "excerpt": "You live in Livermore, California."
    }
  ],
  "untrusted_refs": [
    {
      "ref": "lcm:untrusted:summary:sum_abc123",
      "source": "summary",
      "score": 0.89,
      "created_at": "2026-03-05T00:00:00Z"
    }
  ],
  "stats": {
    "trusted_count": 1,
    "untrusted_count": 1,
    "trusted_chars": 38,
    "limit": 5
  }
}
```

Security invariants:
- `trusted_hits[*].excerpt` contains plain text and is planner-safe.
- `untrusted_refs[*]` MUST NOT include raw text fields.
- `query --lane untrusted|both` returns metadata/refs only for untrusted.

### `expand`

Resolve an untrusted opaque ref for quarantined/qLLM processing.

```bash
sieve-lcm-cli expand \
  --ref lcm:untrusted:summary:sum_abc123 \
  --json
```

Arguments:
- `--untrusted-db <path>`: optional; defaults from `SIEVE_LCM_UNTRUSTED_DB_PATH` then `~/.sieve/lcm/untrusted.db`.
- `--conversation <id>`: optional; defaults from `SIEVE_LCM_GLOBAL_SESSION_ID` then `global`.
- `--ref <opaque_ref>`: ref from `query.untrusted_refs[*].ref`.
- `--json`: required.

Output JSON schema:

```json
{
  "conversation": "global",
  "ref": "lcm:untrusted:sum_abc123",
  "content": "...untrusted text payload...",
  "meta": {
    "source": "summary",
    "id": "sum_abc123"
  }
}
```

Notes:
- `expand` is intended for quarantined/qLLM use only.
- Planner should not receive `content` directly; `sieve-v3` persists it as a ref artifact.

### `ingest`

Append one message to one lane.

```bash
sieve-lcm-cli ingest \
  --db <path> \
  --conversation global \
  --role user \
  --content "hello" \
  --json
```

Arguments:
- `--db <path>`: lane db path.
- `--conversation <id>`: defaults to `global`.
- `--role <user|assistant|system|tool>`.
- `--content <text>`: message content.
- `--json`: required.

Output JSON schema:

```json
{
  "ok": true,
  "conversation": "global"
}
```

## Error Model

All commands return non-zero exit on failure and emit JSON if `--json` is set:

```json
{
  "ok": false,
  "error": {
    "code": "invalid_ref",
    "message": "reference not found"
  }
}
```

## Versioning

- Additive JSON fields are allowed.
- Renames/removals require a new contract version and coordinated update.
