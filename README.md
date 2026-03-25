# sieve

A general-purpose agent that is resistant to prompt injection by design, in Rust.

Currently pre-Alpha.
Use at your own risk.

Inspired by:
- [Simon Willison](https://simonwillison.net/search/?q=prompt-injection)
- [CaMeL](https://arxiv.org/abs/2503.18813)
- [CaMeLs CUA](https://arxiv.org/abs/2601.09923)
- [FIDES](https://arxiv.org/abs/2505.23643)

## Dependencies

You need a Rust toolchain plus a `.env` copied from `.env.example`.
The minimum setup is `OPENAI_API_KEY`; Telegram usage also needs `TELEGRAM_BOT_TOKEN` and `TELEGRAM_CHAT_ID`.
Optional host tools include `trash` for safer file deletion, `st` for voice-note I/O, `codex` for image OCR, and `sieve-lcm-cli` for LCM memory flows.

Full dependency, env, runtime, logging, Telegram, troubleshooting, and external CLI repo links live in [docs/running.md](docs/running.md#running).

## Running

Start from `.env.example`, then set either `OPENAI_API_KEY` or Codex auth with `cargo run -p sieve-app -- auth login openai-codex`.
The most common long-running extras are `SIEVE_PLANNER_MODEL`, `TELEGRAM_BOT_TOKEN`, and `TELEGRAM_CHAT_ID`.

Send a one-off request with:

```bash
cargo run -p sieve-app -- run --prompt "review workspace status"
```

Start long-running mode with:

```bash
cargo run -p sieve-app -- run
```

Long-running mode also accepts automation commands on stdin or Telegram.

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
The container defaults to `HOME=/home/sieve`, `SIEVE_HOME=/home/sieve/.sieve`, and a non-root `1000:1000` runtime user.
Create the host state dir as the real user before first run so Docker does not create it as `root`.

Pull once:

```bash
docker pull nick1udwig/sieve:latest
```

Run one prompt against the current checkout:

```bash
HOST_HOME="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)" && if [ -n "${SUDO_USER:-}" ]; then sudo -u "$SUDO_USER" mkdir -p "$HOST_HOME/.sieve"; else mkdir -p "$HOST_HOME/.sieve"; fi && docker run --rm -it --security-opt seccomp=unconfined --env-file .env -v "$PWD:/workspace" -v "$HOST_HOME/.sieve:/home/sieve/.sieve" nick1udwig/sieve:latest run --prompt "review workspace status"
```

Run long-lived mode:

```bash
HOST_HOME="$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)" && if [ -n "${SUDO_USER:-}" ]; then sudo -u "$SUDO_USER" mkdir -p "$HOST_HOME/.sieve"; else mkdir -p "$HOST_HOME/.sieve"; fi && docker run --rm -it --security-opt seccomp=unconfined --env-file .env -v "$PWD:/workspace" -v "$HOST_HOME/.sieve:/home/sieve/.sieve" nick1udwig/sieve:latest run
```

`seccomp` is Docker's Linux syscall filter.
`--security-opt seccomp=unconfined` disables the default filter so `bubblewrap` and `strace` work inside the container.

## Ansible

If you want a host install that looks like the Docker runtime, use [`ansible/runtime-like-docker.yml`](ansible/runtime-like-docker.yml).
It provisions an Ubuntu host with the same broad runtime shape, then copies in a prebuilt `dist/release` bundle.
Build the bundle with `scripts/build-release-bundle.sh --arch <amd64|arm64> --out-dir dist/release`, then run `ansible-playbook -i <inventory> ansible/runtime-like-docker.yml`.
More detail on what it installs and the host-arch requirement lives in [docs/running.md](docs/running.md#ansible-host-setup).

Full Docker details, bundled CLI tools, and release automation live in [docs/running.md](docs/running.md#docker).
