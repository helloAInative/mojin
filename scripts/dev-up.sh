#!/usr/bin/env bash
# 启动 mj-server（开发模式，本地 cargo run）
#
# 用法：
#   ./scripts/dev-up.sh                  # 后台跑 release
#   ./scripts/dev-up.sh --fg             # 前台跑，看实时日志
#   ./scripts/dev-up.sh --rebuild        # 强制 rebuild + 启动
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$HERE/mj-server"

FG=0
REBUILD=0
for arg in "$@"; do
  case "$arg" in
    --fg)     FG=1 ;;
    --rebuild) REBUILD=1 ;;
  esac
done

# sandbox 隔离规避（cargo target 不要写到 /var/folders）
unset CARGO_TARGET_DIR
export CARGO_TARGET_DIR="$HERE/target"

# 默认端口 / DB
: "${MJ_PORT:=8787}"
export MJ_HOST="127.0.0.1"
export MJ_PORT
export MJ_DB_PATH="${MJ_DB_PATH:-$HERE/data/mojin.db}"
mkdir -p "$(dirname "$MJ_DB_PATH")"

LOG="/tmp/mj-server.log"
PIDFILE="/tmp/mj-server.pid"

# 已有一个在跑？
if [[ -f "$PIDFILE" ]] && kill -0 "$(cat "$PIDFILE")" 2>/dev/null; then
  echo "[dev-up] mj-server already up, pid=$(cat "$PIDFILE"), log=$LOG"
  exit 0
fi
rm -f "$PIDFILE"

# 不接管来源不明的监听进程，避免把旧服务误报为本次启动成功。
OCCUPIED_PID="$(lsof -nP -t -iTCP:"$MJ_PORT" -sTCP:LISTEN 2>/dev/null | head -1 || true)"
if [[ -n "$OCCUPIED_PID" ]]; then
  echo "[dev-up] port $MJ_PORT is already used by pid=$OCCUPIED_PID; stop it before starting mj-server"
  exit 1
fi

if (( REBUILD )); then
  echo "[dev-up] cleaning release build…"
  cargo clean --release
fi

if (( FG )); then
  echo "[dev-up] building + running in foreground (Ctrl-C to stop)"
  exec cargo run --release
fi

echo "[dev-up] building (incremental)…"
cargo build --release

echo "[dev-up] starting mj-server, log=$LOG"
nohup "$CARGO_TARGET_DIR/release/mj-server" </dev/null >"$LOG" 2>&1 &
PID=$!
echo $PID >"$PIDFILE"

# 等到配置端口开始监听
for i in {1..30}; do
  sleep 0.5
  if ! kill -0 "$PID" 2>/dev/null; then
    echo "[dev-up] mj-server exited before listening; tail $LOG"
    tail -30 "$LOG"
    rm -f "$PIDFILE"
    exit 1
  fi
  if lsof -nP -a -p "$PID" -iTCP:"$MJ_PORT" -sTCP:LISTEN 2>/dev/null | grep -q LISTEN; then
    echo "[dev-up] ready on http://127.0.0.1:$MJ_PORT  pid=$PID"
    curl -sS "http://127.0.0.1:$MJ_PORT/healthz" | head -c 200; echo
    exit 0
  fi
done
echo "[dev-up] server did not come up in 15s; tail $LOG"
tail -30 "$LOG"
exit 1
