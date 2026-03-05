# Worker 3 Status: `sieve-command-summaries`

Date: 2026-02-26
Worker: 3
Owned crate: `crates/sieve-command-summaries`

## 2026-03-04 update

- Added `codex exec` command summary support (explicitly not `codex app-server`).
- Added planner catalog entry for `codex` with read-only/workspace-write usage hints.
- `codex exec` summary behavior:
  - always requires `net.connect(https://api.openai.com/)`.
  - `--sandbox read-only` requires `--ephemeral`; output-file flag is rejected.
  - `--sandbox workspace-write` requires `fs.write` for `--cd` scope (or `.` when omitted) plus every `--add-dir` scope.
  - supports `--image` in both read-only and workspace-write modes.
- Added codex-specific unit tests (crate total now 52 tests).

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
- Expanded mutating command mappings:
  - `rm -rf TARGET` (and `sudo rm -rf TARGET`) -> `fs.write(TARGET)`
  - `cp SRC DST` -> `fs.write(DST)`
  - `mv SRC DST` -> `fs.write(SRC) + fs.write(DST)`
  - `mkdir PATH` -> `fs.write(PATH)`
  - `touch PATH` -> `fs.write(PATH)`
  - `chmod MODE PATH...` -> `fs.write(PATH...)`
  - `chown OWNER PATH...` -> `fs.write(PATH...)`
  - `tee FILE...` -> `fs.write(FILE...)`; `tee -a FILE...` -> `fs.append(FILE...)`
- Expanded curl sink mappings:
  - mutating methods: `POST | PUT | PATCH | DELETE` -> `net.write(URL)`
  - payload flags (`-d`, `--data*`, `--json`) emit sink checks to canonicalized URL
  - payload without explicit `-X/--request` defaults to POST handling
  - supports `--url`/`--url=...` and strict header parsing (`-H`/`--header`)
- Unsupported-flag routing tightened:
  - for `rm`, `cp`, `mv`, `mkdir`, `touch`, `chmod`, `chown`, `tee`, `curl`
  - unsupported flags route to `unknown` with `summary.unsupported_flags`
- URL sink canonicalization for curl sink keys:
  - lower-case scheme/host
  - drop query/fragment
  - default port elision (`http:80`, `https:443`)
  - dot-segment normalization
  - percent-encoding normalization; decode unreserved only
- IPv6 host sinks now keep bracket form (`https://[addr]/...`) for canonical key parity.
- Tests expanded to 28 total:
  - mutating command capability coverage
  - curl method/payload/url/header parsing coverage
  - explicit unknown routing for `curl -T` and `curl -F`
  - URL vectors for IDN/punycode host, IPv6 host, encoded slash, root-path default, trailing slash + dot-segments
  - unsupported flag routing
  - Codex parity checks for `bash -lc` safe/dangerous classes
  - URL canonicalization vectors
- Cross-crate follow-up tests added:
  - `sieve-policy`: relative fs scope normalization against `cwd` for capability match/reason output
  - `sieve-runtime`: real summarizer + real policy tests for `cp` capability enforcement and unsupported curl unknown routing

## Surprises

- Cargo fetch blocked in sandbox (network + cargo git cache perms). Needed escalated test run.
- Canonicalization bug found by test:
  - non-default port `:8443` dropped accidentally.
  - fixed by explicit default-port table per scheme.
- Codex dangerous classifier marks `rm -f` as dangerous; kept conservative unknown routing.

## Remaining TODO (within/near Worker 3)

- Add explicit summary support for curl upload/form paths if product wants known handling (currently intentionally unknown with tests).
- Optional: add property/fuzz coverage for URL canonicalization.
