#!/usr/bin/env bash
# 停掉 dev-up.sh 启动的 mj-server。
set -euo pipefail
PIDFILE="/tmp/mj-server.pid"
if [[ ! -f "$PIDFILE" ]]; then
  echo "[dev-down] no pidfile; nothing to do."
  exit 0
fi
PID="$(cat "$PIDFILE")"
if kill -0 "$PID" 2>/dev/null; then
  kill "$PID"
  for _ in {1..20}; do
    sleep 0.25
    if ! kill -0 "$PID" 2>/dev/null; then break; fi
  done
  if kill -0 "$PID" 2>/dev/null; then
    echo "[dev-down] force kill -9 $PID"
    kill -9 "$PID" || true
  fi
fi
rm -f "$PIDFILE"
echo "[dev-down] stopped."