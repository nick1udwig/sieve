#!/usr/bin/env bash
set -euo pipefail
cargo test -p sieve-runtime --test e2e_approval_gate approval_gate_blocks_until_resolution -- --exact --nocapture
