//! 自选股 / 指数池 维护 API。
//!
//! 表 `watchlist(group_name, code)` 主键约束；
//! 表 `index_universe(code)` 主键约束。

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::AppState;
use crate::error::AppError;

// ─────────── watchlist ───────────

#[derive(Debug, Clone, Serialize)]
pub struct WatchItem {
    pub group_name: String,
    pub code: String,
    pub name: String,
    pub note: String,
    pub added_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchAdd {
    #[serde(default)]
    pub group: Option<String>,
    pub code: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

pub fn list_watch(state: &AppState, group: Option<&str>) -> Result<Vec<WatchItem>, AppError> {
    state.with_conn(|c| {
        let (sql, has_g) = match group {
            Some(_) => (
                "SELECT group_name, code, name, note, added_at FROM watchlist WHERE group_name = ? ORDER BY added_at DESC",
                true,
            ),
            None => (
                "SELECT group_name, code, name, note, added_at FROM watchlist ORDER BY group_name, added_at DESC",
                false,
            ),
        };
        let mut stmt = c.prepare(sql)?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<WatchItem> {
            Ok(WatchItem {
                group_name: r.get(0)?,
                code: r.get(1)?,
                name: r.get(2)?,
                note: r.get(3)?,
                added_at: r.get(4)?,
            })
        };
        let rows = if has_g {
            stmt.query_map(params![group.unwrap()], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map([], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    })
}

pub fn add_watch(state: &AppState, req: WatchAdd) -> Result<WatchItem, AppError> {
    if req.code.trim().is_empty() {
        return Err(AppError::Msg("code is required".into()));
    }
    let group = req.group.unwrap_or_else(|| "默认".to_string());
    let name = req.name.unwrap_or_default();
    let note = req.note.unwrap_or_default();
    state.with_conn(|c| {
        c.execute(
            "INSERT INTO watchlist(group_name, code, name, note) VALUES(?,?,?,?)
             ON CONFLICT(group_name, code) DO UPDATE SET
               name=excluded.name,
               note=excluded.note,
               added_at=datetime('now')",
            params![group, req.code, name, note],
        )?;
        let row = c.query_row(
            "SELECT group_name, code, name, note, added_at FROM watchlist WHERE group_name = ? AND code = ?",
            params![group, req.code],
            |r| {
                Ok(WatchItem {
                    group_name: r.get(0)?,
                    code: r.get(1)?,
                    name: r.get(2)?,
                    note: r.get(3)?,
                    added_at: r.get(4)?,
                })
            },
        )?;
        Ok(row)
    })
}

pub fn del_watch(state: &AppState, group: &str, code: &str) -> Result<usize, AppError> {
    state.with_conn(|c| {
        let n = c.execute(
            "DELETE FROM watchlist WHERE group_name = ? AND code = ?",
            params![group, code],
        )?;
        Ok(n)
    })
}

pub fn list_groups(state: &AppState) -> Result<Vec<String>, AppError> {
    state.with_conn(|c| {
        let mut stmt =
            c.prepare("SELECT DISTINCT group_name FROM watchlist ORDER BY group_name")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

// ─────────── index_universe ───────────

#[derive(Debug, Clone, Serialize)]
pub struct IndexItem {
    pub code: String,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexAdd {
    pub code: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

pub fn list_index(state: &AppState, enabled_only: bool) -> Result<Vec<IndexItem>, AppError> {
    state.with_conn(|c| {
        let sql = if enabled_only {
            "SELECT code, name, enabled FROM index_universe WHERE enabled = 1 ORDER BY code"
        } else {
            "SELECT code, name, enabled FROM index_universe ORDER BY code"
        };
        let mut stmt = c.prepare(sql)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(IndexItem {
                    code: r.get(0)?,
                    name: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn add_index(state: &AppState, req: IndexAdd) -> Result<IndexItem, AppError> {
    if req.code.trim().is_empty() {
        return Err(AppError::Msg("code is required".into()));
    }
    let name = req.name.unwrap_or_default();
    let enabled = req.enabled.unwrap_or(true);
    state.with_conn(|c| {
        c.execute(
            "INSERT INTO index_universe(code, name, enabled) VALUES(?,?,?)
             ON CONFLICT(code) DO UPDATE SET
               name=excluded.name,
               enabled=excluded.enabled",
            params![req.code, name, enabled as i64],
        )?;
        let row = c.query_row(
            "SELECT code, name, enabled FROM index_universe WHERE code = ?",
            params![req.code],
            |r| {
                Ok(IndexItem {
                    code: r.get(0)?,
                    name: r.get(1)?,
                    enabled: r.get::<_, i64>(2)? != 0,
                })
            },
        )?;
        Ok(row)
    })
}

pub fn del_index(state: &AppState, code: &str) -> Result<usize, AppError> {
    state.with_conn(|c| {
        let n = c.execute("DELETE FROM index_universe WHERE code = ?", params![code])?;
        Ok(n)
    })
}
