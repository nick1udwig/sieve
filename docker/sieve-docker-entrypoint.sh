#!/usr/bin/env bash
set -euo pipefail

codex_session_name="codex"
sieve_session_name="sieve"
runtime_cwd="${SIEVE_RUNTIME_CWD:-/workspace}"
codex_ws_url="${SIEVE_CODEX_APP_SERVER_WS_URL:-ws://127.0.0.1:4500}"

if [[ $# -eq 0 ]]; then
    set -- run
fi

if [[ "$codex_ws_url" != ws://* ]]; then
    echo "SIEVE_CODEX_APP_SERVER_WS_URL must be a ws:// listener inside the Docker image, got: $codex_ws_url" >&2
    exit 64
fi

mkdir -p "${SIEVE_HOME:-/home/sieve/.sieve}"
export SIEVE_CODEX_APP_SERVER_WS_URL="$codex_ws_url"

shell_join() {
    local joined
    printf -v joined '%q ' "$@"
    printf '%s' "${joined% }"
}

restart_session() {
    local session_name="$1"
    local session_cwd="$2"
    local session_command="$3"
    if tmux has-session -t "$session_name" 2>/dev/null; then
        tmux kill-session -t "$session_name"
    fi
    tmux new-session -d -s "$session_name" -c "$session_cwd" "$session_command"
}

pipe_session_logs() {
    local session_name="$1"
    local log_prefix="$2"
    tmux pipe-pane -o -t "${session_name}:0.0" "sed -u 's/^/[${log_prefix}] /' >&2" || true
}

restart_session \
    "$codex_session_name" \
    "$runtime_cwd" \
    "$(shell_join codex app-server --listen "$codex_ws_url")"
pipe_session_logs "$codex_session_name" "$codex_session_name"

restart_session \
    "$sieve_session_name" \
    "$runtime_cwd" \
    "$(shell_join sieve-app "$@")"
pipe_session_logs "$sieve_session_name" "$sieve_session_name"

if [[ -t 0 && -t 1 && "${SIEVE_DOCKER_NO_ATTACH:-0}" != "1" ]]; then
    tmux attach-session -t "$sieve_session_name" || true
fi

while tmux has-session -t "$sieve_session_name" 2>/dev/null; do
    sleep 1
done
