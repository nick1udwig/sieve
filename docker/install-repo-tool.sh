#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "usage: $0 <owner/repo> <bin-name> [asset-regex]" >&2
    exit 64
fi

repo_slug="$1"
bin_name="$2"
asset_regex="${3:-}"
install_root="${INSTALL_ROOT:-/opt/sieve-tools}"
target_arch="${TARGETARCH:-amd64}"
work_dir="$(mktemp -d)"
api_response="$work_dir/release.json"
download_path="$work_dir/download"
extract_dir="$work_dir/extract"

cleanup() {
    rm -rf "$work_dir"
}

trap cleanup EXIT

mkdir -p "$install_root"
mkdir -p "$extract_dir"

arch_regex() {
    case "$target_arch" in
        amd64) printf '%s\n' 'amd64|x86_64' ;;
        arm64) printf '%s\n' 'arm64|aarch64' ;;
        *)
            echo "unsupported TARGETARCH: $target_arch" >&2
            exit 1
            ;;
    esac
}

release_asset_url() {
    local platform_regex
    platform_regex="$(arch_regex)"

    curl -fsSL -H "Accept: application/vnd.github+json" "https://api.github.com/repos/${repo_slug}/releases/latest" -o "$api_response"

    if [[ -n "$asset_regex" ]]; then
        jq -r --arg asset_regex "$asset_regex" '
            .assets[]
            | select(.name | test($asset_regex; "i"))
            | .browser_download_url
        ' "$api_response" | head -n 1
        return
    fi

    jq -r --arg bin_name "$bin_name" --arg platform_regex "$platform_regex" '
        .assets[]
        | select(
            (.name | test("(linux|unknown-linux|musl|gnu)"; "i"))
            and (.name | test($platform_regex; "i"))
            and (
                (.name | test("\\.(tar\\.gz|tgz|tar\\.xz|zip)$"; "i"))
                or (.name | test("^" + $bin_name + "([._-].+)?$"; "i"))
            )
        )
        | .browser_download_url
    ' "$api_response" | head -n 1
}

install_downloaded_asset() {
    local asset_url="$1"
    local asset_name candidate

    asset_name="${asset_url##*/}"
    curl -fsSL "$asset_url" -o "$download_path"

    case "$asset_name" in
        *.tar.gz|*.tgz)
            tar -xzf "$download_path" -C "$extract_dir"
            ;;
        *.tar.xz)
            tar -xJf "$download_path" -C "$extract_dir"
            ;;
        *.zip)
            unzip -q "$download_path" -d "$extract_dir"
            ;;
        *)
            cp "$download_path" "$extract_dir/$asset_name"
            ;;
    esac

    candidate="$(
        find "$extract_dir" -type f \( -name "$bin_name" -o -name "${bin_name}-*" -o -name "${bin_name}_*" \) \
            ! -name '*.sha256' \
            ! -name '*.sha256sum' \
            ! -name '*.txt' \
            | head -n 1
    )"

    if [[ -z "$candidate" ]]; then
        echo "failed to locate ${bin_name} in asset ${asset_name}" >&2
        find "$extract_dir" -maxdepth 3 -type f >&2 || true
        exit 1
    fi

    install -Dm755 "$candidate" "$install_root/bin/$bin_name"
}

asset_url="$(release_asset_url)"
if [[ -z "$asset_url" ]]; then
    echo "failed to find a matching release asset for ${repo_slug} (${bin_name})" >&2
    jq -r '.assets[]?.name' "$api_response" >&2 || true
    exit 1
fi

install_downloaded_asset "$asset_url"
