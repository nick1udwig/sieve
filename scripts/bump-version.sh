#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
usage: scripts/bump-version.sh [--current|--next-patch|--write|--check]
EOF
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workspace_manifest="${repo_root}/Cargo.toml"
tool_contracts_manifest="${repo_root}/crates/sieve-tool-contracts/Cargo.toml"
workspace_lockfile="${repo_root}/Cargo.lock"
tool_contracts_lockfile="${repo_root}/crates/sieve-tool-contracts/Cargo.lock"

read_workspace_version() {
    awk '
        /^\[workspace\.package\]/ { in_section = 1; next }
        /^\[/ { in_section = 0 }
        in_section && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' "$workspace_manifest"
}

require_semver() {
    local version="$1"
    if [[ ! "$version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        echo "invalid semver: $version" >&2
        exit 1
    fi
}

next_patch_version() {
    local version="$1"
    require_semver "$version"
    local major minor patch
    IFS=. read -r major minor patch <<<"$version"
    printf '%s.%s.%s\n' "$major" "$minor" "$((patch + 1))"
}

set_workspace_version() {
    local next_version="$1"
    local tmp_file
    tmp_file="$(mktemp)"
    awk -v next_version="$next_version" '
        /^\[workspace\.package\]/ { in_section = 1 }
        /^\[/ && $0 != "[workspace.package]" { in_section = 0 }
        in_section && $1 == "version" {
            print "version = \"" next_version "\""
            next
        }
        { print }
    ' "$workspace_manifest" >"$tmp_file"
    cp "$tmp_file" "$workspace_manifest"
    rm -f "$tmp_file"
}

workspace_packages_file() {
    local packages_file
    packages_file="$(mktemp)"
    awk '
        /^name = "/ {
            value = $0
            sub(/^name = "/, "", value)
            sub(/"$/, "", value)
            print value
        }
    ' "$repo_root"/crates/*/Cargo.toml >"$packages_file"
    printf '%s\n' "$packages_file"
}

rewrite_lockfile_versions() {
    local lockfile="$1"
    local next_version="$2"
    local packages_file tmp_file
    packages_file="$(workspace_packages_file)"
    tmp_file="$(mktemp)"
    awk -v next_version="$next_version" -v packages_file="$packages_file" '
        BEGIN {
            while ((getline pkg < packages_file) > 0) {
                packages[pkg] = 1
            }
        }
        /^\[\[package\]\]/ {
            in_package = 1
            update_version = 0
            print
            next
        }
        in_package && /^name = "/ {
            name = $0
            sub(/^name = "/, "", name)
            sub(/"$/, "", name)
            update_version = (name in packages)
            print
            next
        }
        in_package && update_version && /^version = "/ {
            print "version = \"" next_version "\""
            next
        }
        { print }
    ' "$lockfile" >"$tmp_file"
    cp "$tmp_file" "$lockfile"
    rm -f "$tmp_file" "$packages_file"
}

check_lockfile_sync() {
    local lockfile="$1"
    local workspace_version="$2"
    local packages_file
    packages_file="$(workspace_packages_file)"
    local status
    if awk -v workspace_version="$workspace_version" -v packages_file="$packages_file" '
        BEGIN {
            while ((getline pkg < packages_file) > 0) {
                packages[pkg] = 1
            }
        }
        /^\[\[package\]\]/ {
            in_package = 1
            selected = 0
            next
        }
        in_package && /^name = "/ {
            name = $0
            sub(/^name = "/, "", name)
            sub(/"$/, "", name)
            selected = (name in packages)
            next
        }
        in_package && selected && /^version = "/ {
            version = $0
            sub(/^version = "/, "", version)
            sub(/"$/, "", version)
            if (version != workspace_version) {
                exit 1
            }
        }
    ' "$lockfile"; then
        status=0
    else
        status=$?
    fi
    rm -f "$packages_file"
    return "$status"
}

sync_lockfiles() {
    local next_version="$1"
    rewrite_lockfile_versions "$workspace_lockfile" "$next_version"
    rewrite_lockfile_versions "$tool_contracts_lockfile" "$next_version"
}

check_sync() {
    local workspace_version
    workspace_version="$(read_workspace_version)"
    require_semver "$workspace_version"

    if ! grep -Eq '^version\.workspace = true$' "$tool_contracts_manifest"; then
        echo "crates/sieve-tool-contracts/Cargo.toml must inherit workspace version" >&2
        exit 1
    fi

    local mismatches
    mismatches="$(
        find "$repo_root/crates" -name Cargo.toml -print \
            | while IFS= read -r manifest; do
                if [[ "$manifest" == "$tool_contracts_manifest" ]]; then
                    continue
                fi
                if grep -Eq '^version = "[0-9]+\.[0-9]+\.[0-9]+"$' "$manifest"; then
                    printf '%s\n' "$manifest"
                fi
            done
    )"
    if [[ -n "$mismatches" ]]; then
        echo "crate manifests must use version.workspace = true:" >&2
        echo "$mismatches" >&2
        exit 1
    fi

    if ! check_lockfile_sync "$workspace_lockfile" "$workspace_version"; then
        echo "Cargo.lock workspace package versions are out of sync" >&2
        exit 1
    fi

    if ! check_lockfile_sync "$tool_contracts_lockfile" "$workspace_version"; then
        echo "crates/sieve-tool-contracts/Cargo.lock package versions are out of sync" >&2
        exit 1
    fi
}

command="${1:---current}"

case "$command" in
    --current)
        version="$(read_workspace_version)"
        require_semver "$version"
        printf '%s\n' "$version"
        ;;
    --next-patch)
        version="$(read_workspace_version)"
        require_semver "$version"
        next_patch_version "$version"
        ;;
    --write)
        version="$(read_workspace_version)"
        require_semver "$version"
        next_version="$(next_patch_version "$version")"
        set_workspace_version "$next_version"
        sync_lockfiles "$next_version"
        check_sync
        printf '%s\n' "$next_version"
        ;;
    --check)
        check_sync
        ;;
    -h|--help)
        usage
        ;;
    *)
        usage >&2
        exit 64
        ;;
esac
