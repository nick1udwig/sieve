# Worker 3 Status: `sieve-command-summaries`

Date: 2026-02-26
Worker: 3
Owned crate: `crates/sieve-command-summaries`

## Implemented

- Added pinned Codex snapshot dependency:
  - `codex-shell-command` from `https://github.com/openai/codex.git`
  - `rev = 79d6f80`
- Implemented concrete summarizer:
  - `DefaultCommandSummarizer`
  - `CommandSummarizer::summarize`
- Baseline classification from Codex snapshot:
  - `is_known_safe_command`
  - `command_might_be_dangerous`
- MVP command mappings:
  - `rm -rf TARGET` (and `sudo rm -rf TARGET`) -> known summary; `fs.write(TARGET)`
  - `curl -X POST URL` -> known summary; `net.write(URL)`
  - `curl -X POST URL -d BODY` (also `--data*`, `--json`) -> sink check for payload args to URL sink
  - safe read-ish commands (Codex safe class) -> known empty summary
- Unsupported flag handling:
  - `rm`/`curl` unsupported flags route to `unknown`
  - captured in `summary.unsupported_flags`
- URL sink canonicalization implemented for curl POST sink keys:
  - lower-case scheme/host
  - drop query/fragment
  - default port elision (`http:80`, `https:443`)
  - dot-segment normalization
  - percent-encoding normalization; decode unreserved only
- Tests added (11 total), including:
  - required MVP mappings
  - payload sink extraction `ValueRef("argv:<idx>")`
  - unsupported flag routing
  - Codex parity checks for `bash -lc` safe/dangerous classes
  - URL canonicalization vectors

## Surprises

- Cargo fetch blocked in sandbox (network + cargo git cache perms). Needed escalated test run.
- Canonicalization bug found by test:
  - non-default port `:8443` dropped accidentally.
  - fixed by explicit default-port table per scheme.
- Codex dangerous classifier marks `rm -f` as dangerous too; current explicit mapping still only `rm -rf` class per MVP requirement.

## Remaining TODO (within/near Worker 3)

- Expand explicit dangerous summaries beyond `rm -rf` class if policy/runtime wants finer reasons per command class.
- Add more curl method/flag coverage (`PUT`, `PATCH`, multipart/file upload) or intentionally keep unknown with explicit tests.
- Add property/vector tests for URL canonicalization edge cases (Unicode host punycode, tricky percent-encoding paths).
- Wire summaries into integration tests with `sieve-shell` + `sieve-policy` once cross-crate phase starts.
