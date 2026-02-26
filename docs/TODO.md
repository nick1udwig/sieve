# Sieve v3 Worker TODO Board

Primary plan: [mvp-completion-plan.md](/root/git/sieve-v3/docs/mvp-completion-plan.md)

## How Workers Use This Board

1. Pick one item from **Unclaimed**.
2. Move that exact line to **Claimed**.
3. Fill in:
   - `Owner: <name>`
   - `Branch: <branch>`
4. When complete and merged, move line to **Done** and set:
   - `PR: <link-or-id>`
   - check the checkbox (`[x]`).
5. Do not edit items owned by someone else unless reassigned.

## Unclaimed



## Claimed

- [ ] `K` End-to-End Security Harness
: Owner: `codex` | Branch: `master` | PR: `-` | Plan: [Chunk K](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-k-end-to-end-security-harness)

## Done

- [x] `J` Telegram Adapter Finalization
: Owner: `codex` | Branch: `master` | PR: `854d338, 222505e` | Plan: [Chunk J](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-j-telegram-adapter-finalization)

- [x] `I` Runtime End-to-End Planner Loop
: Owner: `codex` | Branch: `master` | PR: `64083ae, 9d50794` | Plan: [Chunk I](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-i-runtime-end-to-end-planner-loop)

- [x] `C` Planner Regeneration Pass
: Owner: `codex` | Branch: `master` | PR: `672eed6, f202525, 273a247` | Plan: [Chunk C](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-c-planner-regeneration-pass)

- [x] `B` Rust Tool Contracts + Schema Emission
: Owner: `codex` | Branch: `master` | PR: `f2b9305, f5c6e9a` | Plan: [Chunk B](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-b-rust-tool-contracts--schema-emission)

- [x] `G` Policy Uses Runtime Context
: Owner: `codex` | Branch: `master` | PR: `1f88c16` | Plan: [Chunk G](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-g-policy-uses-runtime-context)

- [x] `E` Command Summary Expansion
: Owner: `codex` | Branch: `master` | PR: `e5ca790, e6483d0, 192e33f` | Plan: [Chunk E](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-e-command-summary-expansion)

- [x] `F` Runtime Value-State Engine
: Owner: `codex` | Branch: `master` | PR: `7aafd7d` | Plan: [Chunk F](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-f-runtime-value-state-engine)

- [x] `D` Shell Parser Security-Equivalent Parity
: Owner: `codex` | Branch: `master` | PR: `033a8aa, 028c11d` | Plan: [Chunk D](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-d-shell-parser-security-equivalent-parity)

- [x] `H` Quarantine Connect Coverage + Mandatory Report
: Owner: `codex` | Branch: `master` | PR: `d50ef93, 9d2e6b1` | Plan: [Chunk H](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-h-quarantine-connect-coverage--mandatory-report)

- [x] `A` Contract Freeze v1
: Owner: `codex` | Branch: `master` | PR: `a37bde1` | Plan: [Chunk A](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-a-contract-freeze-v1)

- [x] Telegram manual smoke check
: Owner: `integrator` | Branch: `-` | PR: `-` | Notes: sample approval event sent, long-poll wait confirmed.

- [x] Root env template added
: Owner: `integrator` | Branch: `-` | PR: `-` | File: [`.env.example`](/root/git/sieve-v3/.env.example)
