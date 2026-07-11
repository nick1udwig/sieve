# sieve

A general-purpose agent that is resistant to prompt injection by design, in Rust.

Currently pre-Alpha.
Use at your own risk.

Inspired by:
- [Simon Willison](https://simonwillison.net/search/?q=prompt-injection)
- [CaMeL](https://arxiv.org/abs/2503.18813)
- [CaMeLs CUA](https://arxiv.org/abs/2601.09923)
- [FIDES](https://arxiv.org/abs/2505.23643)

## Install

On Linux or macOS x86-64 or ARM64, install the latest release binary and start guided setup with:

```bash
curl -fsSL https://raw.githubusercontent.com/nick1udwig/sieve/master/install.sh | bash
```

The installer detects the host, verifies the release checksum, installs `sieve`, reports available capabilities, offers to install useful system packages, and opens the model-provider wizard.
Run `sieve doctor` at any time for the same capability report.
Only model authentication is required for useful agent turns; Telegram, Git history, quarantine, Codex, browser automation, web-search, audio, and memory CLIs are optional enhancements.
Missing tools are hidden from the planner, and missing quarantine support changes unknown-command handling to deny-by-default.

To build from source instead, install Rust and run `cargo build --release -p sieve-app`.
Full install, dependency, environment, runtime, and troubleshooting details live in [docs/running.md](docs/running.md#running).

## Running

Guided setup offers an OpenAI API key with pay-as-you-go billing or OpenAI Codex through a ChatGPT subscription.
It stores provider and model selection in `$SIEVE_HOME/.env`, stores Codex OAuth separately in `$SIEVE_HOME/state/auth.json`, and masks API-key input when the terminal supports it.
The default `SIEVE_HOME` is `~/.sieve`.
Telegram ingress is enabled only when both `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID` are configured.

Send a one-off request with:

```bash
sieve run --prompt "review workspace status"
```

Start long-running mode with:

```bash
sieve run
```

Long-running mode also accepts automation commands on stdin or Telegram.
For a source checkout, replace `sieve` with `cargo run -p sieve-app --` in these examples.

## Architecture

Sieve keeps the planner isolated from raw untrusted tool output.
The main workspace split is:
- `sieve-app` for the integrated entrypoint,
- `sieve-runtime` for orchestration and approvals,
- `sieve-policy` for capability checks,
- `sieve-quarantine` for sandboxed tracing, and
- supporting crates for summaries, LLM wiring, Telegram, schemas, and shared types.

Architecture notes and crate map: [docs/running.md](docs/running.md#architecture).
Deeper design docs: [docs/sieve-v3-mvp-spec-v1.3.md](docs/sieve-v3-mvp-spec-v1.3.md) and [docs/sieve-v3-mvp-security.md](docs/sieve-v3-mvp-security.md).

Prompts live in `.md` files and get `include_str!`d into the appropriate `.rs` files.
See, e.g., `crates/sieve-app/src/prompts/` and `crates/sieve-llm/src/prompts`.

## Testing

Run the local suite with:

```bash
cargo test --workspace
```

Run the deterministic app harness with:

```bash
cargo test -p sieve-app e2e_fake_ -- --nocapture
```

Commands and coverage notes: [docs/running.md](docs/running.md#testing).

## Docker

A multi-arch Docker image is published to Docker Hub as `nick1udwig/sieve`.
The container defaults to `HOME=/home/sieve`, `SIEVE_HOME=/home/sieve/.sieve`, `CODEX_HOME=/home/sieve/.sieve/state/codex-home`, a non-root `1000:1000` runtime user, and `SIEVE_CODEX_APP_SERVER_WS_URL=ws://127.0.0.1:4500`.
Startup creates tmux session `codex` for `codex app-server` and tmux session `sieve` for `sieve-app`.
Create the host state dir as the real user before first run so Docker does not create it as `root`.
Docker startup loads `/workspace/.env` before either tmux session starts, so the safest Docker path is to mount the repo and not pass `--env-file`.
If `CODEX_HOME/auth.json` is missing, Docker startup mirrors Sieve Codex auth from `$SIEVE_HOME/state/auth.json` into `$CODEX_HOME/auth.json` so the shared Codex app-server can authenticate.
If you do use `--env-file`, keep scalar values unquoted because Docker keeps quotes literal.
If you want Codex to work on a repo other than the mounted `$PWD`, mount that repo too and set `SIEVE_RUNTIME_CWD` to the mounted path inside the container.
`run --prompt` is one-shot mode; it skips Telegram ingress, does not require Telegram env, and exits after printing the answer.

Pull once:

```bash
docker pull nick1udwig/sieve:latest
```

Run one prompt against the current checkout:

```bash
HOST_HOME="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)" && if [ -n "${SUDO_USER:-}" ]; then sudo -u "$SUDO_USER" mkdir -p "$HOST_HOME/.sieve"; else mkdir -p "$HOST_HOME/.sieve"; fi && docker run --rm -it --name sieve-local --security-opt seccomp=unconfined -v "$PWD:/workspace" -v "$HOST_HOME/.sieve:/home/sieve/.sieve" nick1udwig/sieve:latest run --prompt "review workspace status"
```

Run long-lived mode:

```bash
HOST_HOME="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)" && if [ -n "${SUDO_USER:-}" ]; then sudo -u "$SUDO_USER" mkdir -p "$HOST_HOME/.sieve"; else mkdir -p "$HOST_HOME/.sieve"; fi && docker run -d --restart unless-stopped --name sieve-local --security-opt seccomp=unconfined -v "$PWD:/workspace" -v "$HOST_HOME/.sieve:/home/sieve/.sieve" nick1udwig/sieve:latest
```

Run against another repo mounted from `~/git`:

```bash
HOST_HOME="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)" && if [ -n "${SUDO_USER:-}" ]; then sudo -u "$SUDO_USER" mkdir -p "$HOST_HOME/.sieve"; else mkdir -p "$HOST_HOME/.sieve"; fi && docker run --rm -it --name sieve-local --security-opt seccomp=unconfined -e SIEVE_RUNTIME_CWD=/worktrees/sieve-x-scrape -v "$HOME/git:/worktrees" -v "$HOST_HOME/.sieve:/home/sieve/.sieve" nick1udwig/sieve:latest run --prompt "review workspace status"
```

`seccomp` is Docker's Linux syscall filter.
`--security-opt seccomp=unconfined` disables the default filter so `bubblewrap` and `strace` work inside the container.
Docker defaults to foreground logs without auto-attaching tmux.
Set `-e SIEVE_DOCKER_ATTACH=1` if you want startup to attach directly to the `sieve` tmux session.
Inspect tmux sessions with `docker exec -it sieve-local tmux list-sessions`.
Attach to Sieve with `docker exec -it sieve-local tmux attach -t sieve`.
Attach to the shared Codex app-server with `docker exec -it sieve-local tmux attach -t codex`.
Reply to a Codex status card in Telegram with plain text to steer the active Codex turn.
Reply `yes <note>`, `approve <note>`, or `always <note>` to a Codex approval message to resolve approval and pass the note through to Codex in the same turn.

## Ansible

If you want a host install that looks like the Docker runtime, use [`ansible/runtime-like-docker.yml`](ansible/runtime-like-docker.yml).
It provisions an Ubuntu host with the same broad runtime shape, then copies in a prebuilt `dist/release` bundle.
Build the bundle with `scripts/build-release-bundle.sh --arch <amd64|arm64> --out-dir dist/release`, then run `ansible-playbook -i <inventory> ansible/runtime-like-docker.yml`.
More detail on what it installs and the host-arch requirement lives in [docs/running.md](docs/running.md#ansible-host-setup).

Full Docker details, bundled CLI tools, and release automation live in [docs/running.md](docs/running.md#docker).
