use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::error::AppError;
use crate::ws::WsBroker;

pub struct AppState {
    pub db_path: PathBuf,
    conn: Mutex<Connection>,
    pub ws: WsBroker,
}

impl AppState {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let conn = Connection::open(path).map_err(AppError::Db)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(AppError::Db)?;
        Ok(Self {
            db_path: path.to_path_buf(),
            conn: Mutex::new(conn),
            ws: WsBroker::new(),
        })
    }

    pub fn migrate(&self) -> Result<(), AppError> {
        let sql = include_str!("../migrations/001_init.sql");
        let conn = self.conn.lock().map_err(|_| AppError::Lock)?;
        // 旧数据库的 audit_log 可能缺列，必须先补列再执行包含初始化记录的 SQL。
        let _ = conn.execute(
            "ALTER TABLE audit_log ADD COLUMN target TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE audit_log ADD COLUMN payload_json TEXT NOT NULL DEFAULT '{}'",
            [],
        );
        conn.execute_batch(sql).map_err(AppError::Db)?;
        // 兼容老 DB：缺列就补
        let _ = conn.execute(
            "ALTER TABLE signal ADD COLUMN price REAL NOT NULL DEFAULT 0",
            [],
        );
        drop(conn);
        crate::paper::reconcile_all(self)
    }

    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let conn = self.conn.lock().map_err(|_| AppError::Lock)?;
        f(&conn)
    }

    pub fn schema_version(&self) -> Result<String, AppError> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT value FROM schema_meta WHERE key = 'version'",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::Db)
        })
    }

    pub fn paper_equity(&self) -> Result<f64, AppError> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT equity FROM paper_account WHERE id = 'default'",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::Db)
        })
    }

    pub fn table_count(&self) -> Result<i64, AppError> {
        self.with_conn(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )
            .map_err(AppError::Db)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_accepts_old_audit_table_and_is_idempotent() {
        let state = AppState::open(Path::new(":memory:")).unwrap();
        state
            .with_conn(|c| {
                c.execute_batch(
                    "CREATE TABLE audit_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    actor TEXT NOT NULL DEFAULT 'system',
                    action TEXT NOT NULL,
                    detail TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL DEFAULT (datetime('now'))
                );",
                )?;
                Ok(())
            })
            .unwrap();
        state.migrate().unwrap();
        state.migrate().unwrap();
        assert_eq!(state.schema_version().unwrap(), "4");
        let init_count: i64 = state
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM audit_log WHERE action = 'init' AND target = 'mojin-v4'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(init_count, 1);
    }
}
