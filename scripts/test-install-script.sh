#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_root="$(mktemp -d)"

cleanup() {
    rm -rf "$tmp_root"
}

trap cleanup EXIT

case "$(uname -m)" in
    x86_64|amd64) asset="sieve-linux-x86_64" ;;
    aarch64|arm64) asset="sieve-linux-aarch64" ;;
    *) echo "unsupported test architecture" >&2; exit 1 ;;
esac

release_dir="$tmp_root/releases/latest/download"
install_dir="$tmp_root/bin"
mkdir -p "$release_dir"

cp "$repo_root/scripts/fixtures/fake-sieve-release" "$release_dir/$asset"
chmod +x "$release_dir/$asset"
(
    cd "$release_dir"
    sha256sum "$asset" > checksums.txt
)

SIEVE_INSTALL_BASE_URL="file://$tmp_root/releases" bash "$repo_root/install.sh" --prefix "$install_dir" --skip-setup --yes

test -x "$install_dir/sieve"
test -L "$install_dir/sieve-app"
"$install_dir/sieve" doctor --json | grep -q '"core_ready":true'

SIEVE_INSTALL_BASE_URL="file://$tmp_root/releases" bash "$repo_root/install.sh" --prefix "$install_dir" --skip-setup --yes

test -x "$install_dir/sieve"
