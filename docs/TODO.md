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

- [ ] `AA` Telegram Full-Flow Harness (Mock Long-Poll + Outbound Assertions)
: Owner: `unclaimed` | Branch: `-` | PR: `-` | Scope: implement a full app/adapter harness that mocks Telegram `getUpdates`, injects user messages, and asserts outbound `sendMessage`/typing behavior through the real flow. Include deterministic and env-gated live cases from [telegram-fullflow-harness-plan.md](/root/git/sieve-v3/docs/telegram-fullflow-harness-plan.md), specifically greeting + `weather in dublin ireland today` + `weather in dublin ireland tomorrow`.

## Claimed

## Done

- [x] `AB` Complete CLI Search Migration (Remove Dedicated `brave_search` Tool Path)
: Owner: `codex` | Branch: `master` | PR: `2045452` | Scope: fully removed dedicated `brave_search` contracts/types/runtime/app paths; kept only backward-compatible filtering in `parse_allowed_tools`; updated tests and docs for bash/CLI-driven search.

- [x] `Y` Modality Parity Contract (Reply In Same Mode As Input)
: Owner: `codex` | Branch: `master` | PR: `5f1d5cd` | Scope: define and enforce a modality contract so input modality is tracked (`text|audio|image|...`) and response defaults to same modality unless explicitly overridden by policy/tool failure. Apply this across Telegram ingress, runtime turn context, and response delivery. Files: [lib.rs](/root/git/sieve-v3/crates/sieve-types/src/lib.rs), [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs), [adapter.rs](/root/git/sieve-v3/crates/sieve-interface-telegram/src/adapter.rs), [README.md](/root/git/sieve-v3/README.md). Done when: modality parity is explicit in types/docs and covered by integration tests.

- [x] `X` Image Input Pipeline (Vision/OCR -> Act)
: Owner: `codex` | Branch: `master` | PR: `4b6a461` | Scope: support Telegram image/photo input, run OCR/recognition through configured vision path (vLLM-compatible), and feed results into normal agent flow without exposing unsafe raw strings across planner boundary. Prefer non-hardcoded behavior via tool/ref architecture (emergent handling), with documented dependencies/config and tests. Files: [adapter.rs](/root/git/sieve-v3/crates/sieve-interface-telegram/src/adapter.rs), [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs), [lib.rs](/root/git/sieve-v3/crates/sieve-llm/src/lib.rs), [README.md](/root/git/sieve-v3/README.md), [`.env.example`](/root/git/sieve-v3/.env.example). Done when: image-origin turns are processed end-to-end with policy-safe handling and documented setup.

- [x] `W` Audio I/O Pipeline (Voice Note -> STT -> Act -> TTS)
: Owner: `codex` | Branch: `master` | PR: `69b9c65` | Scope: support Telegram voice-note input, convert to text via STT, run normal agent flow, then reply in audio (TTS) for audio-origin turns. Preserve trust semantics: transcription inherits trust level of source voice note (trusted for approved sender in current model). Prefer non-hardcoded implementation where modality handling emerges from available bash tools + refs; document required host tools (for example `ffmpeg`, STT CLI/runtime, TTS CLI/runtime) and fallback behavior. Files: [adapter.rs](/root/git/sieve-v3/crates/sieve-interface-telegram/src/adapter.rs), [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs), [README.md](/root/git/sieve-v3/README.md), [`.env.example`](/root/git/sieve-v3/.env.example). Done when: voice-note turn works end-to-end and response mode matches input mode.

- [x] `V` Telegram Typing Indicator During Turn Execution
: Owner: `codex` | Branch: `master` | PR: `e5fc882` | Scope: while a turn is being processed, emit Telegram `typing` chat action and reliably stop on success, model/tool failure, or cancellation so typing does not get stuck. Include tests around lifecycle behavior. Files: [adapter.rs](/root/git/sieve-v3/crates/sieve-interface-telegram/src/adapter.rs), [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs). Done when: typing is visible during active work and always stops/cleans up correctly.

- [x] `U` Brave Web Search Tool Integration
: Owner: `codex` | Branch: `master` | PR: `2319db5` | Scope: add a planner-callable Brave web search tool (OpenAI planner compatible) with typed/structured tool output, policy enforcement, and response-writer integration. Include env/docs wiring for Brave credentials/config and tests for allow/deny + execution path. Files: [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs), [lib.rs](/root/git/sieve-v3/crates/sieve-runtime/src/lib.rs), [lib.rs](/root/git/sieve-v3/crates/sieve-types/src/lib.rs), [README.md](/root/git/sieve-v3/README.md), [`.env.example`](/root/git/sieve-v3/.env.example). Done when: planner can invoke Brave search end-to-end under policy, and tests/docs/env are updated.

- [x] `R` Live End-to-End Smoke (Real OpenAI + Telegram + Execution)
: Owner: `codex` | Branch: `master` | PR: `bd19e85` | Scope: run one full live flow with real credentials and policy via `sieve-app` (planner call, approval over Telegram, and real mainline command execution). Files: [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs), [README.md](/root/git/sieve-v3/README.md), [`.env.example`](/root/git/sieve-v3/.env.example). Done when: run evidence is recorded (input command, approval action, resulting command exit/report, and event log path). Evidence: input prompt `Use bash to run exactly: mkdir -p /tmp/sieve-r-live-smoke`; approval command `/approve_once approval-1`; result `ExecuteMainline` with exit `0`; event log `.sieve/logs/runtime-events-r-20260226-231218.jsonl`. Rerun (2026-02-26): input prompt `Use bash to run exactly: mkdir -p /tmp/sieve-r-live-smoke-rerun`; approval command `/approve_once approval-1`; result `ExecuteMainline` with exit `0`; event log `/tmp/sieve-e2e-20260226-234424/logs/runtime-events.jsonl` with `conversation` + runtime events.

- [x] `T` Q-LLM Runtime Decision and Integration (If Required)
: Owner: `codex` | Branch: `master` | PR: `04de1aa` | Scope: explicitly decide whether Q-LLM typed outputs must be in the integrated runtime loop for MVP. If yes, wire typed guidance model into app/runtime flow and add integration tests; if no, document deferral in MVP docs/TODO. Files: [main.rs](/root/git/sieve-v3/crates/sieve-app/src/main.rs), [openai.rs](/root/git/sieve-v3/crates/sieve-llm/src/openai.rs), [sieve-v3-mvp-spec-v1.3.md](/root/git/sieve-v3/docs/sieve-v3-mvp-spec-v1.3.md), [docs/TODO.md](/root/git/sieve-v3/docs/TODO.md). Done when: decision is explicit and corresponding code/tests/docs are aligned.

- [x] `S` Baseline Policy File + Wiring
: Owner: `codex` | Branch: `master` | PR: `c03bebf` | Scope: add a checked-in baseline policy TOML and wire docs/env defaults to that path so `SIEVE_POLICY_PATH` has an in-repo default target. Files: [`.env.example`](/root/git/sieve-v3/.env.example), [README.md](/root/git/sieve-v3/README.md), plus new policy file under `docs/` or repo root. Done when: fresh setup can point to a committed policy file without out-of-band file creation.

- [x] `Q` End-to-End Tests for Remaining MVP Blockers
: Owner: `codex` | Branch: `master` | PR: `29967ba` | Scope: add integration tests that lock behavior for `L` through `P` (runtime allowlist gate, explicit-tool policy gate, mainline execution, unknown/uncertain policy events, integrated entrypoint wiring). Files: [e2e_security_policy_flows.rs](/root/git/sieve-v3/crates/sieve-runtime/tests/e2e_security_policy_flows.rs), [e2e_security_quarantine_modes.rs](/root/git/sieve-v3/crates/sieve-runtime/tests/e2e_security_quarantine_modes.rs), plus new tests as needed.

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
