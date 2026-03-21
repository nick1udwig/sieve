#!/usr/bin/env bash
set -euo pipefail

playbook="ansible/runtime-like-docker.yml"

require() {
    local pattern="$1"
    local message="$2"
    if ! grep -Eq "$pattern" "$playbook"; then
        echo "$message" >&2
        exit 1
    fi
}

require '^  vars:$' "missing vars section"
require 'scripts/build-release-bundle\.sh --arch' "playbook docs must reference the release bundle build command"
require 'https://deb\.nodesource\.com/node_\{\{ sieve_node_major \}\}\.x' "playbook must install Node 22 from NodeSource"
require 'dest: /usr/local/bin/sieve-app' "playbook must install sieve-app"
require 'dest: "\{\{ sieve_tools_root \}\}/"' "playbook must install the prebuilt tools bundle"
require 'dest: /etc/profile\.d/sieve\.sh' "playbook must install shell defaults"
require '/opt/sieve-tools/bin/codex' "playbook must verify the Codex CLI"
