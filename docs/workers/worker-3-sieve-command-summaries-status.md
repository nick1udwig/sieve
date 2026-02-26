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
- Tests expanded to 21 total:
  - mutating command capability coverage
  - curl method/payload/url/header parsing coverage
  - unsupported flag routing
  - Codex parity checks for `bash -lc` safe/dangerous classes
  - URL canonicalization vectors

## Surprises

- Cargo fetch blocked in sandbox (network + cargo git cache perms). Needed escalated test run.
- Canonicalization bug found by test:
  - non-default port `:8443` dropped accidentally.
  - fixed by explicit default-port table per scheme.
- Codex dangerous classifier marks `rm -f` as dangerous; kept conservative unknown routing.

## Remaining TODO (within/near Worker 3)

- Add more curl upload coverage (`--upload-file`, multipart/form) or keep unknown with explicit tests.
- Add property/vector tests for URL canonicalization edge cases (Unicode host punycode, tricky percent-encoding paths).
- Wire summaries into integration tests with `sieve-shell` + `sieve-policy` once cross-crate phase starts.
