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

- [ ] `Q` End-to-End Tests for Remaining MVP Blockers
: Owner: `codex` | Branch: `master` | PR: `-` | Scope: add integration tests that lock behavior for `L` through `P` (runtime allowlist gate, explicit-tool policy gate, mainline execution, unknown/uncertain policy events, integrated entrypoint wiring). Files: [e2e_security_policy_flows.rs](/root/git/sieve-v3/crates/sieve-runtime/tests/e2e_security_policy_flows.rs), [e2e_security_quarantine_modes.rs](/root/git/sieve-v3/crates/sieve-runtime/tests/e2e_security_quarantine_modes.rs), plus new tests as needed.

## Done

- [x] `P` Runnable App Entrypoint (Runtime + OpenAI + Telegram)
: Owner: `codex` | Branch: `master` | PR: `35b1359` | Scope: add a production binary entrypoint wiring runtime loop with OpenAI planner/quarantine, approval bus, event log, Telegram adapter, and command execution path. Files: [Cargo.toml](/root/git/sieve-v3/Cargo.toml), [manual-smoke.rs](/root/git/sieve-v3/crates/sieve-interface-telegram/examples/manual-smoke.rs), [openai_live.rs](/root/git/sieve-v3/crates/sieve-llm/examples/openai_live.rs), plus new app files. Done when: one command starts the full integrated system.

- [x] `O` Policy Audit Event Parity for Unknown/Uncertain
: Owner: `codex` | Branch: `master` | PR: `5281759` | Scope: emit `PolicyEvaluated` events for unknown/uncertain `deny|ask|accept` paths, not only known-command precheck path. Files: [lib.rs](/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs). Done when: all decision paths produce policy audit entries.

- [x] `N` Mainline Command Execution Path
: Owner: `codex` | Branch: `master` | PR: `9f538c4` | Scope: implement actual execution for approved/allowed mainline Bash commands (current path returns disposition only). Files: [lib.rs](/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs). Done when: `ExecuteMainline` runs command segments and reports outcome.

- [x] `M` Policy Gate for `endorse` and `declassify`
: Owner: `codex` | Branch: `master` | PR: `1293af5` | Scope: add explicit policy evaluation for `endorse`/`declassify` before approval and state mutation. Files: [lib.rs](/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs), [lib.rs](/root/git/sieve-v3/crates/sieve-policy/src/lib.rs), [lib.rs](/root/git/sieve-v3/crates/sieve-types/src/lib.rs). Done when: flow is `policy -> approval -> transition`, with deny paths covered.

- [x] `L` Runtime Allowed-Tools Enforcement Boundary
: Owner: `codex` | Branch: `master` | PR: `3caedf3` | Scope: enforce `PlannerRunRequest.allowed_tools` in runtime dispatch so disallowed `tool_name` values are rejected before execution. Files: [lib.rs](/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs). Done when: runtime rejects disallowed tools even if planner backend omits allowlist checks.

- [x] `K` End-to-End Security Harness
: Owner: `codex` | Branch: `master` | PR: `710d409, 026eff3, d1e839d` | Plan: [Chunk K](/root/git/sieve-v3/docs/mvp-completion-plan.md#chunk-k-end-to-end-security-harness)

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
