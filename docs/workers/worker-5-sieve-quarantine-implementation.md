# Worker 5 Implementation: `sieve-quarantine`

## Scope Implemented

- Implemented concrete runner in `crates/sieve-quarantine/src/lib.rs`:
  - `BwrapQuarantineRunner`
  - `QuarantineRunner::run`
- Quarantine execution path:
  - `bwrap` sandbox
  - `--unshare-net` (no network)
  - `strace -ff -s 4096 -o <run_dir>/strace`
- Artifact layout:
  - `~/.sieve/logs/traces/<run_id>/`
  - `stdout.log`
  - `stderr.log`
  - `strace*`
- Report output:
  - returns `QuarantineReport`
  - fills `trace_path`, `stdout_path`, `stderr_path`, `exit_code`
  - parses and normalizes `attempted_capabilities`

## Trace Parsing Implemented

- `execve` / `execveat` -> `proc.exec`
- `open` / `openat` / `openat2` / `creat` -> `fs.read|write|append`
- mutating FS ops (`unlink`, `rename`, `mkdir`, etc.) -> `fs.write`
- `connect`:
  - `AF_INET` / `AF_INET6` -> `net.connect`
  - `AF_UNIX` -> `ipc.connect`
- dedupe + stable ordering for report output

## Tests Added

- command segment reconstruction test (composition operators)
- trace parse normalization + dedupe test
- path layout test (`<logs_root>/<run_id>`)
- fake-bwrap smoke test (artifacts + report fields)
- missing-trace regression test (clear error on failed sandbox run)

## Validation Run

- `cargo test --manifest-path crates/sieve-quarantine/Cargo.toml` -> pass (5 tests)
- `rustfmt --edition 2021 --check crates/sieve-quarantine/src/lib.rs` -> pass
- Manual smoke (real tools):
  - installed `bubblewrap`
  - ran `bwrap + strace -ff`
  - verified trace artifacts under:
    - `/root/.sieve/logs/traces/manual-smoke-1772130977/`

## Surprises

- Workspace-level test command became unstable later:
  - `cargo test -p sieve-quarantine` blocked by unrelated manifest issue in `crates/sieve-llm/Cargo.toml` (no target)
  - used crate-manifest scoped tests to continue validation
- Environment initially lacked `bwrap`; needed package install before real smoke
- `Resource`/`Action` in `sieve-types` not `Ord`; capability dedupe adjusted to numeric sort keys

## Remaining TODO (Worker 5 scope-adjacent)

- optional: broaden syscall coverage for attempted capability extraction (`sendto`, `bind`, `socket`, env-affecting syscalls)
- optional: write machine-readable per-run summary artifact (e.g. `report.json`) alongside traces
- integration: wire runner into runtime event flow (Worker 7 ownership)
