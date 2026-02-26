# Scope: Quarantine Syscall Coverage Expansion

Date: 2026-02-26
Owner lane: `crates/sieve-quarantine`
Related question: syscall coverage and why it exists.

## Objective

Expand and harden syscall-to-capability inference from `strace` artifacts for quarantine runs, improving audit quality and user visibility for unknown/uncertain accepted commands.

## Decisions (Locked)

- `report.json` is **mandatory** whenever trace artifacts exist for a run.
- Scope prioritizes **connect attempts**; no dedicated DNS-attempt feature work.

## Why This Matters

In MVP, unknown/uncertain accepted commands run in quarantine. We capture attempted effects and surface them as inferred capability attempts. Better syscall coverage improves:

- transparency in approval/audit events,
- operator understanding of actual command behavior,
- future policy tuning (without auto-learning in MVP).

## Current Coverage

- Process exec: `execve`, `execveat` -> `proc.exec`
- FS open family: `open/openat/openat2/creat` -> `fs.read|write|append`
- FS mutating ops: `unlink/rename/mkdir/chmod/truncate/...` -> `fs.write`
- Connect:
  - `AF_INET/AF_INET6` -> `net.connect`
  - `AF_UNIX` -> `ipc.connect`

## Proposed Work

1. Add network-related syscall parsing beyond `connect`:
   - `socket`, `sendto`, `sendmsg`, `recvfrom`, `recvmsg`, `bind`, `listen`, `accept*`
   - map to `net.connect` and/or `ipc.connect` with clear heuristics.
2. Add environment/process metadata ops:
   - candidate env access indicators where visible in trace context
   - child process spawn variants (`clone`, `vfork`, `fork`) linked to exec chains.
3. Improve scope normalization:
   - IPv6 formatting consistency
   - unknown address fallback tags
   - path canonicalization rules for FS scope where feasible.
4. Emit per-run normalized summary artifact (mandatory when trace exists):
   - `~/.sieve/logs/traces/<run_id>/report.json`
5. Add golden trace fixtures and parser tests for each syscall family.
6. Improve connect-attempt visibility:
   - preserve/connect endpoint details for `AF_INET`, `AF_INET6`, and `AF_UNIX`
   - record best-effort address/port extraction and fallback tags for unknown endpoints.

## Deliverables

- Expanded parser in `sieve-quarantine`.
- Tests for new syscall mappings and normalization.
- Mandatory JSON report artifact writer (`report.json` when trace exists).
- README section documenting mapping table and known limits.

## Non-Goals

- Runtime enforcement changes.
- Post-exec dynamic policy mutation.
- Side-channel defenses.

## Risks

- `strace` format variability across distributions/kernel versions.
- False positives or over-broad capability attribution.
