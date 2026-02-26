# Sieve v3 TODO

## Setup

- [x] Run Telegram manual smoke test with `.env` values.
: Output showed sample approval event published and sent; process then waited in long-poll loop.
- [x] Add root env template.
: See [`.env.example`](/root/git/sieve-v3/.env.example).

## Scope Docs (Needs Decisions + Signoff)

- [x] Shell parser parity scope decisions captured.
: [scope-shell-parser-parity.md](/root/git/sieve-v3/docs/scope-shell-parser-parity.md)
- [x] Quarantine syscall coverage scope decisions captured.
: [scope-quarantine-syscall-coverage.md](/root/git/sieve-v3/docs/scope-quarantine-syscall-coverage.md)
- [x] Tool contract hardening scope decisions captured.
: [scope-tool-contract-hardening.md](/root/git/sieve-v3/docs/scope-tool-contract-hardening.md)

## Scope Implementation

- [ ] Implement shell parser parity work.
: [scope-shell-parser-parity.md](/root/git/sieve-v3/docs/scope-shell-parser-parity.md)
- [ ] Implement syscall coverage expansion and mandatory trace `report.json`.
: [scope-quarantine-syscall-coverage.md](/root/git/sieve-v3/docs/scope-quarantine-syscall-coverage.md)
- [ ] Implement Rust-type tool contracts + schema generation + one-pass regeneration diagnostics.
: [scope-tool-contract-hardening.md](/root/git/sieve-v3/docs/scope-tool-contract-hardening.md)

## Supporting Docs

- [x] Telegram README includes chat-id retrieval steps.
: [sieve-interface-telegram README](/root/git/sieve-v3/crates/sieve-interface-telegram/README.md)
