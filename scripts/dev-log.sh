#!/usr/bin/env bash
# 看后端日志
#
# 用法：
#   ./scripts/dev-log.sh              # tail -f
#   ./scripts/dev-log.sh --last 200   # 最后 200 行
set -euo pipefail
LOG="${MJ_LOG:-/tmp/mj-server.log}"
if [[ ! -f "$LOG" ]]; then
  echo "[dev-log] no log at $LOG"
  exit 1
fi
if [[ "${1:-}" == "--last" && -n "${2:-}" ]]; then
  tail -n "$2" "$LOG"
else
  tail -n 50 -f "$LOG"
fi