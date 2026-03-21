#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: $0 --arch <amd64|arm64> --out-dir <path>" >&2
}

target_arch=""
out_dir=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --arch)
            target_arch="${2:-}"
            shift 2
            ;;
        --out-dir)
            out_dir="${2:-}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 64
            ;;
    esac
done

case "$target_arch" in
    amd64|arm64)
        ;;
    *)
        usage
        exit 64
        ;;
esac

if [[ -z "$out_dir" ]]; then
    usage
    exit 64
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
codex_npm_spec="${CODEX_NPM_SPEC:-@openai/codex}"
tools_dir="${out_dir}/sieve-tools"

cd "$repo_root"

rm -rf "$out_dir"
mkdir -p "$out_dir"

cargo build --locked --release -p sieve-app

install -Dm755 target/release/sieve-app "${out_dir}/sieve-app"

npm install --global --prefix "$tools_dir" "$codex_npm_spec"

TARGETARCH="$target_arch" INSTALL_ROOT="$tools_dir" docker/install-repo-tool.sh nick1udwig/brave-search bravesearch "${BRAVE_SEARCH_ASSET_REGEX:-}"
TARGETARCH="$target_arch" INSTALL_ROOT="$tools_dir" docker/install-repo-tool.sh nick1udwig/st st "${ST_ASSET_REGEX:-}"
TARGETARCH="$target_arch" INSTALL_ROOT="$tools_dir" docker/install-repo-tool.sh nick1udwig/sieve-lcm sieve-lcm-cli "${SIEVE_LCM_ASSET_REGEX:-}"
