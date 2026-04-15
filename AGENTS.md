# AGENTS.MD

Work style: telegraph; noun-phrases ok; drop grammar; min tokens.

## Agent Protocol
- Repos live at https://github.com/nick1udwig
- Company repos live at https://github.com/hive-agents
- Contact: Nick Ludwig (@nick1udwig, nick.b.ludwig@gmail.com).
- Workspace: `~/git`
- `~/ops`: private ops (domains/DNS, redirects/workers, runbooks).
- Find hosts via `~/.ssh/config`.
- Files: repo or `~/git/agent-scripts`.
- “Make a note” => edit AGENTS.md (shortcut; not a blocker). Ignore `CLAUDE.md`.
- Need upstream file: stage in `/tmp/`, then cherry-pick; never overwrite tracked.
- Bugs: first add regression test, then fix, using test to confirm.
- Keep files <~500 LOC; split/refactor as needed.
- Commits: Conventional Commits (`feat|fix|refactor|build|ci|chore|docs|style|perf|test`).
- Editor: `vi <path>`.
- Prefer end-to-end verify; if blocked, say what’s missing.
- New deps: quick health check (recent releases/commits, adoption).
- Web: search early; quote exact errors; prefer 2024–2026 sources; fallback Firecrawl (`pnpm mcp:*`) / `mcporter`.
- Oracle: run `npx -y @steipete/oracle --help` once/session before first use.
- Style: telegraph. Drop filler/grammar. Min tokens (global AGENTS + replies).
- Frontend: always auto-detect light/dark mode and serve proper one; just autodetect & serve don't put a toggle button

## Screenshots (“use a screenshot”)
- Pick newest PNG in `~/Pictures`.
- Verify it’s the right UI (ignore filename).
- Replace asset; keep dimensions; run gate; commit.

## Important Locations
- Notes/Runbooks: `~/ops/docs/`
- Notes: `~/Documents` (prefixed with year like `26-hyperware`)

## Docs
- Start: run docs list (`docs:list` script, or `bin/docs-list` here if present; ignore if not installed); open docs before coding.
- Follow links until domain makes sense; honor `Read when` hints.
- Keep notes short; update docs when behavior/API changes (no ship w/o test & docs).
- Add `read_when` hints on cross-cutting docs.
- Model preference: latest only. OK: Anthropic Opus 4.6, OpenAI GPT-5.4.
- One-liners always on one line, never broken up with `\`s.
- `.md` files: only one sentence per line.

## PR Feedback
- Active PR: `gh pr view --json number,title,url --jq '"PR #\\(.number): \\(.title)\\n\\(.url)"'`.
- PR comments: `gh pr view …` + `gh api …/comments --paginate`.
- Replies: cite fix + file/line; resolve threads only after fix lands.
- When merging a PR: thank the contributor in `CHANGELOG.md`.

## Flow & Runtime
- Use repo’s package manager/runtime; no swaps w/o approval.
- Use Codex background for long jobs; tmux only for interactive/persistent (debugger/server).
- Sieve runtime: tmux session `sieve-app` with command `cargo build --release && SIEVE_PLANNER_MODEL=gpt-5.4 SIEVE_QUARANTINE_MODEL=gpt-5.4 ./target/release/sieve-app`.
- Docker env files: keep scalar `.env` values unquoted; Docker `--env-file` keeps quotes literal. Prefer mounting the repo and letting Sieve load `/workspace/.env`.
- Testing workflow: always look for running sieve process and stop it before tests.
- Post-change workflow: look for and stop sieve, then build and start sieve in `sr`.

## Build / Test
- Before handoff: run full gate (lint/typecheck/tests/docs).
- CI red: `gh run list/view`, rerun, fix, push, repeat til green.
- Keep it observable (logs, panes, tails, MCP/browser tools).
- Release: read `docs/RELEASING.md` (or find best checklist if missing).
- Reminder: check `~/.profile` for missing env keys.

## Git
- Safe by default: `git status/diff/log`. Push only when user asks.
- `git checkout` ok for PR review / explicit request.
- Branch changes require user consent.
- Destructive ops forbidden unless explicit (`reset --hard`, `clean`, `restore`, `rm`, …).
- Remotes under `~/git`: prefer HTTPS for public repos; flip SSH->HTTPS before pull/push for public repos.
- Commit helper on PATH: `committer` (bash). Prefer it; if repo has `./scripts/committer`, use that.
- Don’t delete/rename unexpected stuff; stop + ask.
- No repo-wide S/R scripts; keep edits small/reviewable.
- Avoid manual `git stash`; if Git auto-stashes during pull/rebase, that’s fine (hint, not hard guardrail).
- If user types a command (“pull and push”), that’s consent for that command.
- No amend unless asked.
- Big review: `git --no-pager diff --color=never`.
- Multi-agent: check `git status/diff` before edits; ship small commits.

## Language/Stack Notes
- TypeScript: use repo PM; run `docs:list`; keep files small; follow existing patterns.

## Critical Thinking
- Fix root cause (not band-aid).
- Typed tool args with hidden format rules: avoid opaque strings; prefer typed unions or let runtime parse higher-level natural language.
- Unsure: read more code; if still stuck, ask w/ short options.
- Conflicts: call out; pick safer path.
- Unrecognized changes: assume other agent; keep going; focus your changes. If it causes issues, stop + ask user.
- Leave breadcrumb notes in thread.

## Tools

Read `~/agent-scripts/tools.md` for the full tool catalog if it exists.

### bin/docs-list / scripts/docs-list.ts
- Optional. Lists `docs/` + enforces front-matter. Ignore if `bin/docs-list` not installed. Rebuild: `bun build scripts/docs-list.ts --compile --outfile bin/docs-list`.

### agent-browser
- agent-browser - fast browser automation CLI for AI agents

### gdb
- Use `gdb` inside tmux to debug native apps; attach to the running app to inspect state.

### trash
- Move files to Trash: `trash ...`.

### oracle
- Bundle prompt+files for 2nd model. Use when stuck/buggy/review.
- Run `npx -y @steipete/oracle --help` once/session (before first use).

### mcporter / iterm / firecrawl / XcodeBuildMCP
- MCP launcher: `npx mcporter <server>` (see `npx mcporter --help`). Common: `iterm`, `firecrawl`, `XcodeBuildMCP`.

### gh
- GitHub CLI for PRs/CI/releases. Given issue/PR URL (or `/pull/5`): use `gh`, not web search.
- Examples: `gh issue view <url> --comments -R owner/repo`, `gh pr view <url> --comments --files -R owner/repo`.

### tmux
- Use only when you need persistence/interaction (debugger/server).
- Quick refs: `tmux new -d -s codex-shell`, `tmux attach -t codex-shell`, `tmux list-sessions`, `tmux kill-session -t codex-shell`.

<frontend_aesthetics>
Avoid “AI slop” UI. Be opinionated + distinctive.

Do:
- Typography: pick a real font; avoid Inter/Roboto/Arial/system defaults.
- Theme: commit to a palette; use CSS vars; bold accents > timid gradients.
- Motion: 1–2 high-impact moments (staggered reveal beats random micro-anim).
- Background: add depth (gradients/patterns), not flat default.

Avoid: purple-on-white clichés, generic component grids, predictable layouts.
</frontend_aesthetics>
