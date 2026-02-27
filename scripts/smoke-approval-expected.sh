#!/usr/bin/env bash
set -euo pipefail
cargo test -p sieve-runtime --test e2e_approval_gate approval_gate_allows_execution_after_approve_once -- --exact --nocapture
