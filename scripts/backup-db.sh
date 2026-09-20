#!/usr/bin/env bash
# 备份 SQLite 数据库
#
# 用法：
#   ./scripts/backup-db.sh            # 默认 ./data -> ./data/backups/
#   ./scripts/backup-db.sh /path/to/db  # 指定 db 路径
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
DB="${1:-$HERE/data/mojin.db}"
DEST="$(dirname "$DB")/backups"
mkdir -p "$DEST"

STAMP="$(date +%Y%m%d-%H%M%S)"
NAME="$(basename "$DB" .db)-$STAMP.db"

if command -v sqlite3 >/dev/null; then
  # 在线安全备份：-backup 在源 DB 上加写锁做一致性拷贝
  sqlite3 "$DB" ".backup '$DEST/$NAME'"
else
  # 退化路径：直接 cp
  cp "$DB" "$DEST/$NAME"
fi

# 软链 latest -> 最新
ln -sfn "$NAME" "$DEST/$(basename "$DB" .db)-latest.db"
echo "[backup-db] $DEST/$NAME"

# 保留最近 30 天
find "$DEST" -maxdepth 1 -name 'mojin-*.db' -mtime +30 -delete || true