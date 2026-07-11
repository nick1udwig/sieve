#!/usr/bin/env bash
set -euo pipefail

version="latest"
prefix=""
skip_setup=false
assume_yes=false
dry_run=false
base_url="${SIEVE_INSTALL_BASE_URL:-https://github.com/nick1udwig/sieve/releases}"

usage() {
    echo "usage: install.sh [--version VERSION] [--prefix BIN_DIR] [--skip-setup] [--yes] [--dry-run]"
}

log() {
    printf '→ %s\n' "$1"
}

success() {
    printf '✓ %s\n' "$1"
}

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            [[ $# -ge 2 ]] || fail "--version requires a value"
            version="$2"
            shift 2
            ;;
        --prefix)
            [[ $# -ge 2 ]] || fail "--prefix requires a value"
            prefix="$2"
            shift 2
            ;;
        --skip-setup)
            skip_setup=true
            shift
            ;;
        --yes|-y)
            assume_yes=true
            shift
            ;;
        --dry-run)
            dry_run=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

case "$(uname -s)" in
    Linux) os="linux" ;;
    Darwin) os="darwin" ;;
    *) fail "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) fail "unsupported architecture: $(uname -m)" ;;
esac

asset="sieve-${os}-${arch}"
if [[ "$version" == "latest" ]]; then
    download_root="${base_url}/latest/download"
else
    case "$version" in
        v*) tag="$version" ;;
        *) tag="v${version}" ;;
    esac
    download_root="${base_url}/download/${tag}"
fi

if [[ -z "$prefix" ]]; then
    if [[ "$(id -u)" -eq 0 ]]; then
        prefix="/usr/local/bin"
    else
        prefix="${HOME}/.local/bin"
    fi
fi

log "Detected ${os}/${arch}"
log "Install directory: ${prefix}"

if [[ "$dry_run" == true ]]; then
    printf 'would download %s/%s\n' "$download_root" "$asset"
    printf 'would install %s/sieve\n' "$prefix"
    exit 0
fi

if command -v curl >/dev/null 2>&1; then
    fetch() {
        curl -fsSL "$1" -o "$2"
    }
elif command -v wget >/dev/null 2>&1; then
    fetch() {
        wget -qO "$2" "$1"
    }
else
    fail "curl or wget is required to download Sieve"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
    rm -rf "$tmp_dir"
}
trap cleanup EXIT

log "Downloading ${asset}"
fetch "${download_root}/${asset}" "$tmp_dir/$asset"
fetch "${download_root}/checksums.txt" "$tmp_dir/checksums.txt"

expected="$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$tmp_dir/checksums.txt")"
[[ -n "$expected" ]] || fail "checksums.txt does not contain ${asset}"
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp_dir/$asset" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp_dir/$asset" | awk '{print $1}')"
else
    fail "sha256sum or shasum is required to verify the download"
fi
[[ "$actual" == "$expected" ]] || fail "checksum verification failed for ${asset}"
success "Checksum verified"

install_user_scoped() {
    mkdir -p "$prefix"
    install -m 755 "$tmp_dir/$asset" "$prefix/.sieve.tmp.$$"
    mv "$prefix/.sieve.tmp.$$" "$prefix/sieve"
    ln -sfn sieve "$prefix/sieve-app"
}

if install_user_scoped 2>/dev/null; then
    :
elif command -v sudo >/dev/null 2>&1; then
    if [[ "$assume_yes" != true && -r /dev/tty && -w /dev/tty ]]; then
        printf 'Install Sieve into %s with sudo? [y/N] ' "$prefix" > /dev/tty
        IFS= read -r answer < /dev/tty || answer=""
        case "$answer" in
            y|Y|yes|YES) ;;
            *) fail "installation cancelled" ;;
        esac
    fi
    sudo mkdir -p "$prefix"
    sudo install -m 755 "$tmp_dir/$asset" "$prefix/.sieve.tmp.$$"
    sudo mv "$prefix/.sieve.tmp.$$" "$prefix/sieve"
    sudo ln -sfn sieve "$prefix/sieve-app"
else
    fail "${prefix} is not writable and sudo is unavailable"
fi

success "Installed ${prefix}/sieve"
if [[ ":${PATH}:" != *":${prefix}:"* ]]; then
    printf 'Add %s to PATH to run sieve from a new shell.\n' "$prefix"
fi

if [[ "$skip_setup" == false ]]; then
    if [[ -r /dev/tty && -w /dev/tty ]]; then
        "$prefix/sieve" setup < /dev/tty
    else
        "$prefix/sieve" setup --non-interactive
    fi
fi
