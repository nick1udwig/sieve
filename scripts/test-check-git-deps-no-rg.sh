#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

tmp_bin="$(mktemp -d)"
trap 'rm -rf "$tmp_bin"' EXIT

for cmd in bash dirname git grep head mktemp rm sed; do
  ln -s "$(command -v "$cmd")" "$tmp_bin/$cmd"
done

PATH="$tmp_bin" scripts/check-git-deps.sh >/dev/null
