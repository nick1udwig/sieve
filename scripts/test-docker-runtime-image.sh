#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_root="$(mktemp -d)"
image_tag="sieve-docker-runtime-test:$$-$(date +%s)"
container_name="sieve-docker-runtime-test-$$"

cleanup() {
    docker rm -f "$container_name" >/dev/null 2>&1 || true
    docker image rm -f "$image_tag" >/dev/null 2>&1 || true
    rm -rf "$tmp_root"
}

trap cleanup EXIT

require() {
    local pattern="$1"
    local file="$2"
    local message="$3"
    if ! grep -Eq "$pattern" "$file"; then
        echo "$message" >&2
        exit 1
    fi
}

reject() {
    local pattern="$1"
    local file="$2"
    local message="$3"
    if grep -Eq "$pattern" "$file"; then
        echo "$message" >&2
        exit 1
    fi
}

for dockerfile in "$repo_root/Dockerfile" "$repo_root/Dockerfile.release"; do
    reject 'HOME=/root' "$dockerfile" "$dockerfile must not default HOME to /root"
    reject 'SIEVE_HOME=/root/\.sieve' "$dockerfile" "$dockerfile must not default SIEVE_HOME to /root/.sieve"
    reject '/root/\.sieve' "$dockerfile" "$dockerfile must not reference /root/.sieve"
    reject '^USER[[:space:]]+root([[:space:]]|$)' "$dockerfile" "$dockerfile must not run as root"
    require 'CODEX_HOME=/home/sieve/\.sieve/state/codex-home' "$dockerfile" "$dockerfile must default CODEX_HOME inside SIEVE_HOME"
    require 'HOME=/home/sieve' "$dockerfile" "$dockerfile must default HOME to /home/sieve"
    require 'SIEVE_CODEX_APP_SERVER_WS_URL=ws://127\.0\.0\.1:4500' "$dockerfile" "$dockerfile must default the shared Codex app-server websocket URL"
    require 'SIEVE_HOME=/home/sieve/\.sieve' "$dockerfile" "$dockerfile must default SIEVE_HOME to /home/sieve/.sieve"
    require 'tmux' "$dockerfile" "$dockerfile must install tmux for persistent codex and sieve sessions"
    require 'sieve-docker-entrypoint' "$dockerfile" "$dockerfile must use the Docker tmux entrypoint"
    require '^USER[[:space:]]+(sieve|1000|1000:1000|\$\{SIEVE_UID\}:\$\{SIEVE_GID\})$' "$dockerfile" "$dockerfile must set a non-root runtime user"
done

require 'SIEVE_DOCKER_ATTACH' "$repo_root/docker/sieve-docker-entrypoint.sh" "docker entrypoint must use SIEVE_DOCKER_ATTACH for opt-in tmux attach"
reject 'SIEVE_DOCKER_NO_ATTACH' "$repo_root/docker/sieve-docker-entrypoint.sh" "docker entrypoint must not default to auto-attaching tmux sessions"

build_ctx="$tmp_root/build"
mkdir -p "$build_ctx/docker" "$build_ctx/docs/policy" "$build_ctx/dist/release/sieve-tools/bin"

cp "$repo_root/Dockerfile.release" "$build_ctx/Dockerfile.release"
cp "$repo_root/.env.example" "$build_ctx/.env.example"
cp "$repo_root/docker/sieve-docker-entrypoint.sh" "$build_ctx/docker/sieve-docker-entrypoint.sh"
cp -R "$repo_root/docs/policy/." "$build_ctx/docs/policy/"

cat > "$build_ctx/dist/release/sieve-app" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$SIEVE_HOME"
touch "$SIEVE_HOME/owned-by-runtime" "$SIEVE_HOME/sieve-started"
printf '%s\n' "$*" > "$SIEVE_HOME/sieve-argv"
if [[ "${1:-}" == "auth" ]]; then
    printf 'auth-ok\n'
    exit 0
fi
if [[ "${1:-}" != "run" ]]; then
    exit 0
fi
if [[ "${2:-}" == "--prompt" ]]; then
    printf 'prompt-ok:%s\n' "${3:-}"
    exit 0
fi
while true; do sleep 60; done
EOF
chmod +x "$build_ctx/dist/release/sieve-app"

cat > "$build_ctx/dist/release/sieve-tools/bin/codex" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$SIEVE_HOME"
touch "$SIEVE_HOME/codex-started"
printf '%s\n' "$*" > "$SIEVE_HOME/codex-argv"
printf '%s\n' "${OPENAI_API_KEY:-}" > "$SIEVE_HOME/codex-openai-api-key"
if [[ -f "${CODEX_HOME}/auth.json" ]]; then
    touch "$SIEVE_HOME/codex-auth-present"
fi
while true; do sleep 60; done
EOF
chmod +x "$build_ctx/dist/release/sieve-tools/bin/codex"

for tool in bravesearch st sieve-lcm-cli; do
    cat > "$build_ctx/dist/release/sieve-tools/bin/$tool" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$build_ctx/dist/release/sieve-tools/bin/$tool"
done

docker build -f "$build_ctx/Dockerfile.release" -t "$image_tag" "$build_ctx" >/dev/null

config_user="$(docker image inspect "$image_tag" --format '{{.Config.User}}')"
case "$config_user" in
    ""|root|0|0:0)
        echo "Dockerfile.release must configure a non-root default user" >&2
        exit 1
        ;;
esac

runtime_state="$(docker run --rm --entrypoint /bin/bash "$image_tag" -lc 'printf "%s\n%s\n%s\n%s\n" "$HOME" "$SIEVE_HOME" "$CODEX_HOME" "$(id -u):$(id -g)"')"
runtime_home="$(printf '%s\n' "$runtime_state" | sed -n '1p')"
runtime_sieve_home="$(printf '%s\n' "$runtime_state" | sed -n '2p')"
runtime_codex_home="$(printf '%s\n' "$runtime_state" | sed -n '3p')"
runtime_ids="$(printf '%s\n' "$runtime_state" | sed -n '4p')"

[[ "$runtime_home" == "/home/sieve" ]] || {
    echo "runtime HOME must be /home/sieve, got $runtime_home" >&2
    exit 1
}
[[ "$runtime_sieve_home" == "/home/sieve/.sieve" ]] || {
    echo "runtime SIEVE_HOME must be /home/sieve/.sieve, got $runtime_sieve_home" >&2
    exit 1
}
[[ "$runtime_codex_home" == "/home/sieve/.sieve/state/codex-home" ]] || {
    echo "runtime CODEX_HOME must be /home/sieve/.sieve/state/codex-home, got $runtime_codex_home" >&2
    exit 1
}
[[ "$runtime_ids" == "1000:1000" ]] || {
    echo "runtime user must be 1000:1000, got $runtime_ids" >&2
    exit 1
}

bind_home="$tmp_root/home"
mkdir -p "$bind_home"
mkdir -p "$bind_home/state"
if [[ "$(id -u)" == "0" ]]; then
    chown -R 1000:1000 "$bind_home"
fi
cat > "$bind_home/state/auth.json" <<'EOF'
{
  "openai-codex": {
    "type": "oauth",
    "access": "header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC1kb2NrZXIiLCJjaGF0Z3B0X3BsYW5fdHlwZSI6InBybyIsInVzZXJfaWQiOiJ1c2VyLWRvY2tlciJ9fQ.sig",
    "refresh": "refresh-token",
    "accountId": "acct-docker"
  }
}
EOF
docker run --rm --entrypoint /usr/local/bin/sieve-app -v "$bind_home:/home/sieve/.sieve" "$image_tag" >/dev/null

file_owner="$(stat -c '%u:%g' "$bind_home/owned-by-runtime")"
[[ "$file_owner" == "1000:1000" ]] || {
    echo "runtime must not create root-owned files in SIEVE_HOME, got $file_owner" >&2
    exit 1
}

docker run -d --name "$container_name" -v "$bind_home:/home/sieve/.sieve" "$image_tag" >/dev/null
for _ in $(seq 1 20); do
    [[ -f "$bind_home/codex-started" && -f "$bind_home/sieve-started" ]] && break
    sleep 1
done

[[ -f "$bind_home/codex-started" ]] || {
    echo "runtime must start the codex tmux session" >&2
    exit 1
}
[[ -f "$bind_home/sieve-started" ]] || {
    echo "runtime must start the sieve tmux session" >&2
    exit 1
}
[[ -f "$bind_home/codex-auth-present" ]] || {
    echo "runtime must sync Codex auth into CODEX_HOME before starting the codex tmux session" >&2
    exit 1
}
[[ -f "$bind_home/state/codex-home/auth.json" ]] || {
    echo "runtime must persist generated Codex auth inside SIEVE_HOME/state/codex-home/auth.json" >&2
    exit 1
}
grep -q '"auth_mode": "chatgpt"' "$bind_home/state/codex-home/auth.json" || {
    echo "generated Codex auth must preserve refresh-capable chatgpt mode when sieve auth has a refresh token" >&2
    exit 1
}
grep -q '"account_id": "acct-docker"' "$bind_home/state/codex-home/auth.json" || {
    echo "generated Codex auth must copy the ChatGPT account id from sieve auth" >&2
    exit 1
}

prompt_output="$(docker run --rm -v "$bind_home:/home/sieve/.sieve" "$image_tag" run --prompt "review workspace status" 2>&1)"
printf '%s\n' "$prompt_output" | grep -q 'prompt-ok:review workspace status' || {
    echo "one-shot run --prompt must emit sieve-app output to the container logs" >&2
    exit 1
}
printf '%s\n' "$prompt_output" | grep -q '^\[codex\] ' || {
    echo "one-shot run --prompt must still start the codex app-server session" >&2
    exit 1
}

session_list="$(docker exec "$container_name" tmux list-sessions)"
printf '%s\n' "$session_list" | grep -q '^codex:' || {
    echo "runtime must create a tmux session named codex" >&2
    exit 1
}
printf '%s\n' "$session_list" | grep -q '^sieve:' || {
    echo "runtime must create a tmux session named sieve" >&2
    exit 1
}

codex_path="$(docker exec "$container_name" /bin/bash -lc 'command -v codex')"
[[ "$codex_path" == "/opt/sieve-tools/bin/codex" ]] || {
    echo "runtime must install codex into /opt/sieve-tools/bin, got $codex_path" >&2
    exit 1
}

grep -qx 'app-server --listen ws://127.0.0.1:4500' "$bind_home/codex-argv" || {
    echo "codex tmux session must run the app server on ws://127.0.0.1:4500" >&2
    exit 1
}
grep -qx 'run' "$bind_home/sieve-argv" || {
    echo "sieve tmux session must run sieve-app with the default 'run' command" >&2
    exit 1
}

docker rm -f "$container_name" >/dev/null 2>&1 || true

bind_home_dotenv="$tmp_root/home-dotenv"
workspace_dotenv="$tmp_root/workspace-dotenv"
mkdir -p "$bind_home_dotenv" "$workspace_dotenv"
if [[ "$(id -u)" == "0" ]]; then
    chown -R 1000:1000 "$bind_home_dotenv"
fi
cat > "$workspace_dotenv/.env" <<'EOF'
OPENAI_API_KEY=sk-from-workspace-dotenv
EOF

docker run -d --name "$container_name" -v "$bind_home_dotenv:/home/sieve/.sieve" -v "$workspace_dotenv:/workspace" "$image_tag" >/dev/null
for _ in $(seq 1 20); do
    [[ -f "$bind_home_dotenv/codex-started" ]] && break
    sleep 1
done

grep -qx 'sk-from-workspace-dotenv' "$bind_home_dotenv/codex-openai-api-key" || {
    echo "runtime must load /workspace/.env before starting the codex tmux session" >&2
    exit 1
}
