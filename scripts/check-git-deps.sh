#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

check_git_dep() {
  local name="$1"
  local manifest="$2"
  local repo="$3"
  local line rev

  line="$(grep -E "^[[:space:]]*${name}[[:space:]]*=" "$manifest" | head -n 1 || true)"
  if [[ -z "$line" ]]; then
    echo "missing dependency line for ${name} in ${manifest}" >&2
    exit 1
  fi

  rev="$(printf '%s\n' "$line" | sed -n 's/.*rev = "\([^"]*\)".*/\1/p')"
  if [[ -z "$rev" ]]; then
    echo "missing git rev for ${name} in ${manifest}" >&2
    exit 1
  fi

  local tmp_dir
  tmp_dir="$(mktemp -d)"

  git -C "$tmp_dir" init -q
  if ! git -C "$tmp_dir" fetch --depth 1 "$repo" "$rev" >/dev/null 2>&1; then
    rm -rf "$tmp_dir"
    echo "unreachable git rev for ${name}: ${rev} (${repo})" >&2
    exit 1
  fi

  rm -rf "$tmp_dir"
  echo "ok ${name} ${rev}"
}

check_git_dep "codex-shell-command" "crates/sieve-command-summaries/Cargo.toml" "https://github.com/openai/codex.git"
check_git_dep "sieve-lcm" "crates/sieve-app/Cargo.toml" "https://github.com/nick1udwig/sieve-lcm.git"
