#!/usr/bin/env bash
set -euo pipefail

if [[ -f .env ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
fi

: "${TELEGRAM_BOT_TOKEN:?missing TELEGRAM_BOT_TOKEN (set in env or .env)}"
: "${TELEGRAM_CHAT_ID:?missing TELEGRAM_CHAT_ID (set in env or .env)}"

run_ts="$(date +%s)"
run_slug="live-approval-${run_ts}"
export SIEVE_HOME="${SIEVE_HOME:-/tmp/sieve-${run_slug}}"
mkdir -p "$SIEVE_HOME"
event_log="$SIEVE_HOME/logs/runtime-events.jsonl"
app_out="$SIEVE_HOME/app.out"
target_dir="/tmp/sieve-${run_slug}-target"
mkdir -p "${target_dir}"

prompt="Use bash to run exactly: rm -rf ${target_dir}"
cargo build -p sieve-app >/dev/null
target/debug/sieve-app "${prompt}" >"${app_out}" 2>&1 &
app_pid=$!
trap 'kill "${app_pid}" >/dev/null 2>&1 || true' EXIT

request_id=""
deadline=$((SECONDS + 90))
while ((SECONDS < deadline)); do
  if ! kill -0 "${app_pid}" >/dev/null 2>&1; then
    break
  fi
  if [[ -f "${event_log}" ]]; then
    request_id="$(
      (
        grep '"event":"approval_requested"' "${event_log}" || true
      ) | tail -n1 | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p'
    )"
    if [[ -n "${request_id}" ]]; then
      break
    fi
  fi
  sleep 1
done

if [[ -z "${request_id}" ]]; then
  echo "failed: did not observe approval_requested event"
  echo "app output:"
  cat "${app_out}"
  exit 1
fi

echo "approval request observed: ${request_id}"
echo "approve manually in Telegram chat ${TELEGRAM_CHAT_ID} (reply yes/y, react 👍, or /approve_once ${request_id})"

resolved=""
deadline=$((SECONDS + 180))
while ((SECONDS < deadline)); do
  if [[ -f "${event_log}" ]]; then
    resolved="$(
      (
        grep '"event":"approval_resolved"' "${event_log}" || true
      ) | (
        grep "\"request_id\":\"${request_id}\"" || true
      ) | tail -n1
    )"
    if [[ -n "${resolved}" ]]; then
      break
    fi
  fi
  if ! kill -0 "${app_pid}" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if [[ -z "${resolved}" ]]; then
  echo "failed: approval_resolved not observed; app may still be waiting for your Telegram approval"
  echo "app output:"
  cat "${app_out}"
  exit 1
fi
if ! grep -q '"action":"approve_once"' <<<"${resolved}"; then
  echo "failed: expected approve_once action, got: ${resolved}"
  exit 1
fi

wait "${app_pid}"
trap - EXIT

if [[ -d "${target_dir}" ]]; then
  echo "failed: expected directory removed after approved rm -rf: ${target_dir}"
  cat "${app_out}"
  exit 1
fi
if ! grep -q '"event":"approval_resolved"' "${event_log}"; then
  echo "failed: approval_resolved missing in ${event_log}"
  cat "${event_log}"
  exit 1
fi
if ! grep -q "ExecuteMainline" "${app_out}"; then
  echo "failed: mainline execution not observed in app output"
  cat "${app_out}"
  exit 1
fi

echo "ok: approval flow executed live"
echo "request_id=${request_id}"
echo "sieve_home=${SIEVE_HOME}"
echo "event_log=${event_log}"
echo "app_out=${app_out}"
