#!/usr/bin/env bash
set -euo pipefail

codex_session_name="codex"
sieve_session_name="sieve"
runtime_cwd="${SIEVE_RUNTIME_CWD:-/workspace}"

load_simple_env_file() {
    local env_file="$1"
    [[ -f "$env_file" ]] || return 0
    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%$'\r'}"
        [[ "$line" =~ ^[[:space:]]*$ ]] && continue
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        if [[ "$line" =~ ^[[:space:]]*(export[[:space:]]+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
            export "${BASH_REMATCH[2]}=${BASH_REMATCH[3]}"
        fi
    done < "$env_file"
}

load_simple_env_file "$runtime_cwd/.env"

runtime_cwd="${SIEVE_RUNTIME_CWD:-$runtime_cwd}"
codex_ws_url="${SIEVE_CODEX_APP_SERVER_WS_URL:-ws://127.0.0.1:4500}"
sieve_home="${SIEVE_HOME:-/home/sieve/.sieve}"
codex_home_default="${sieve_home}/state/codex-home"
codex_home="${CODEX_HOME:-$codex_home_default}"
default_codex_home=0
if [[ -z "${CODEX_HOME:-}" || "$codex_home" == "$codex_home_default" ]]; then
    default_codex_home=1
fi

if [[ $# -eq 0 ]]; then
    set -- run
fi

is_prompt_run_mode() {
    [[ "${1:-}" == "run" ]] || return 1
    shift || true
    local arg
    for arg in "$@"; do
        if [[ "$arg" == "--prompt" ]]; then
            return 0
        fi
    done
    return 1
}

is_auth_mode() {
    [[ "${1:-}" == "auth" ]]
}

if [[ "$codex_ws_url" != ws://* ]]; then
    echo "SIEVE_CODEX_APP_SERVER_WS_URL must be a ws:// listener inside the Docker image, got: $codex_ws_url" >&2
    exit 64
fi

export SIEVE_HOME="$sieve_home"
export CODEX_HOME="$codex_home"
export SIEVE_CODEX_APP_SERVER_WS_URL="$codex_ws_url"
mkdir -p "$SIEVE_HOME" "$CODEX_HOME"

shell_join() {
    local joined
    printf -v joined '%q ' "$@"
    printf '%s' "${joined% }"
}

write_codex_auth_json() {
    local access_token="$1"
    local account_id="$2"
    local refresh_token="${3:-}"
    local auth_mode="chatgptAuthTokens"
    local tmp_path="$CODEX_HOME/auth.json.tmp.$$"
    local last_refresh
    last_refresh="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    if [[ -n "$refresh_token" ]]; then
        auth_mode="chatgpt"
    fi
    jq -n \
        --arg auth_mode "$auth_mode" \
        --arg access_token "$access_token" \
        --arg account_id "$account_id" \
        --arg refresh_token "$refresh_token" \
        --arg last_refresh "$last_refresh" \
        '{
            auth_mode: $auth_mode,
            OPENAI_API_KEY: null,
            tokens: {
                id_token: $access_token,
                access_token: $access_token,
                refresh_token: $refresh_token,
                account_id: $account_id
            },
            last_refresh: $last_refresh
        }' > "$tmp_path"
    chmod 600 "$tmp_path"
    mv "$tmp_path" "$CODEX_HOME/auth.json"
}

sync_codex_auth() {
    local codex_auth_path="$CODEX_HOME/auth.json"
    local sieve_auth_path="${SIEVE_OPENAI_CODEX_AUTH_JSON_PATH:-$SIEVE_HOME/state/auth.json}"
    local access_token=""
    local account_id=""
    local refresh_token=""

    if [[ -n "${OPENAI_API_KEY:-}" ]]; then
        return 0
    fi

    if [[ -n "${OPENAI_CODEX_ACCESS_TOKEN:-}" && -n "${OPENAI_CODEX_ACCOUNT_ID:-}" ]]; then
        write_codex_auth_json \
            "$OPENAI_CODEX_ACCESS_TOKEN" \
            "$OPENAI_CODEX_ACCOUNT_ID" \
            "${OPENAI_CODEX_REFRESH_TOKEN:-}"
        return 0
    fi

    if [[ "$default_codex_home" -eq 0 && -s "$codex_auth_path" ]]; then
        return 0
    fi

    if [[ ! -s "$sieve_auth_path" ]]; then
        return 0
    fi

    access_token="$(jq -r '.["openai-codex"].access // empty' "$sieve_auth_path")"
    account_id="$(jq -r '.["openai-codex"].accountId // empty' "$sieve_auth_path")"
    refresh_token="$(jq -r '.["openai-codex"].refresh // empty' "$sieve_auth_path")"

    if [[ -n "$access_token" && -n "$account_id" ]]; then
        write_codex_auth_json "$access_token" "$account_id" "$refresh_token"
    fi
}

sync_codex_auth

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

if ! is_auth_mode "$@"; then
    restart_session \
        "$codex_session_name" \
        "$runtime_cwd" \
        "$(shell_join codex app-server --listen "$codex_ws_url")"
    pipe_session_logs "$codex_session_name" "$codex_session_name"
fi

if is_prompt_run_mode "$@"; then
    cd "$runtime_cwd"
    exec sieve-app "$@"
fi

if is_auth_mode "$@"; then
    cd "$runtime_cwd"
    exec sieve-app "$@"
fi

restart_session \
    "$sieve_session_name" \
    "$runtime_cwd" \
    "$(shell_join sieve-app "$@")"
pipe_session_logs "$sieve_session_name" "$sieve_session_name"

if [[ -t 0 && -t 1 && "${SIEVE_DOCKER_ATTACH:-0}" == "1" ]]; then
    tmux attach-session -t "$sieve_session_name" || true
fi

while tmux has-session -t "$sieve_session_name" 2>/dev/null; do
    sleep 1
done
