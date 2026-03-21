#!/usr/bin/env bash
set -euo pipefail

workflow=".github/workflows/release.yml"

require() {
    local pattern="$1"
    local message="$2"
    if ! grep -Eq "$pattern" "$workflow"; then
        echo "$message" >&2
        exit 1
    fi
}

reject() {
    local pattern="$1"
    local message="$2"
    if grep -Eq "$pattern" "$workflow"; then
        echo "$message" >&2
        exit 1
    fi
}

require '^  prepare:$' "missing prepare job"
require '^  build-image:$' "missing build-image job"
require '^  publish-manifest:$' "missing publish-manifest job"
require 'ubuntu-24\.04-arm' "missing native arm64 runner"
require 'Dockerfile\.release' "release workflow must use Dockerfile.release"
require 'scripts/build-release-bundle\.sh --arch "\$\{\{ matrix\.arch \}\}" --out-dir dist/release' "release workflow must prebuild the release bundle"
require 'Mozilla-Actions/sccache-action@v0\.0\.9' "release workflow must install sccache"
require 'RUSTC_WRAPPER: sccache' "release workflow must compile bundles through sccache"
require 'SCCACHE_GHA_ENABLED: "true"' "release workflow must enable the sccache GitHub cache backend"
require 'SCCACHE_GHA_VERSION: release-bundle-v1-\$\{\{ matrix\.arch \}\}' "release workflow must namespace sccache per arch"
require 'sccache --show-stats \|\| true' "release workflow must report sccache stats"
require 'docker buildx imagetools create -t nick1udwig/sieve:\$\{\{ needs\.prepare\.outputs\.version \}\}' "release workflow must publish a multi-arch version manifest"
require 'docker buildx imagetools create -t nick1udwig/sieve:latest' "release workflow must publish a multi-arch latest manifest"
reject 'setup-qemu-action' "release workflow must not use QEMU"
reject 'platforms: linux/amd64,linux/arm64' "release workflow must not build both architectures in one Docker step"
