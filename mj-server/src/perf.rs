//! 信号后验：5/10/20 日 forward return（不复权口径以落库当日为准；行情取前复权日线）
//!
//! 触发逻辑：
//! - 取 `signal.fired_at` 的**交易日** `anchor`，用 `signal.price` 作为 base。
//! - 在 `fetch_daily_voted` 返回的前复权日线里，定位 `anchor` 的索引，向后取 5/10/20 个交易日的收盘价。
//! - ret_n = close_t / base - 1，写入 `signal_performance`。

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::params;
use serde::Serialize;

use crate::db::AppState;
use crate::error::AppError;
use crate::market::{self, DayBar};

/// 默认观察窗口（含 1d / 3d，便于后续扩展）
pub const DEFAULT_WINDOWS: &[i64] = &[1, 3, 5, 10, 20];

#[derive(Debug, Clone, Serialize)]
pub struct PerfRow {
    pub signal_id: String,
    pub code: String,
    pub level: String,
    pub fired_at: String,
    pub price: f64,
    pub ret_1d: Option<f64>,
    pub ret_3d: Option<f64>,
    pub ret_5d: Option<f64>,
    pub ret_10d: Option<f64>,
    pub ret_20d: Option<f64>,
    pub labeled_at: Option<String>,
    pub pending: Vec<i64>,
}

/// 用 `fired_at`（UTC ISO8601）抽出"日期"，再找日线里与之相同或紧随的交易日作为锚点。
fn locate_anchor(bars: &[DayBar], fired_at: &str) -> Option<usize> {
    let fired_date = parse_fire_date(fired_at)?;
    // 优先匹配同一日期；否则找第一个 >= fired_date 的交易日
    let mut idx_after: Option<usize> = None;
    for (i, b) in bars.iter().enumerate() {
        if b.date == fired_date {
            return Some(i);
        }
        if idx_after.is_none() && b.date.as_str() >= fired_date.as_str() {
            idx_after = Some(i);
        }
    }
    idx_after
}

fn parse_fire_date(s: &str) -> Option<String> {
    // 1) 完整 RFC3339
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(
            dt.with_timezone(&Utc)
                .date_naive()
                .format("%Y-%m-%d")
                .to_string(),
        );
    }
    // 2) 已经是 yyyy-mm-dd
    if NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").is_ok() {
        return Some(s.trim().to_string());
    }
    // 3) 兜底：前 10 字符
    if s.len() >= 10 {
        return Some(s[..10].to_string());
    }
    None
}

/// 计算给定信号在所有窗口下的 forward return。
/// 返回 `PerfRow`，其中没有覆盖到足够天数的窗口置为 `None`，并记录在 `pending` 里。
pub async fn compute_returns(state: &AppState, signal_id: &str) -> Result<PerfRow, AppError> {
    let (code, level, fired_at, price): (String, String, String, f64) = state.with_conn(|c| {
        c.query_row(
            "SELECT code, level, fired_at, price FROM signal WHERE id = ?",
            params![signal_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(AppError::Db)
    })?;

    // 现有 perf（用于保留旧值；如果行情拉空就返回 None 但保留旧 perf）
    let existing: (Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<f64>, Option<String>) =
        state.with_conn(|c| {
            match c.query_row(
                "SELECT ret_1d, ret_3d, ret_5d, ret_10d, ret_20d, labeled_at FROM signal_performance WHERE signal_id = ?",
                params![signal_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            ) {
                Ok(v) => Ok(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok((
                    None, None, None, None, None, None,
                )),
                Err(e) => Err(AppError::Db(e)),
            }
        })?;

    // 拉日线：取最长窗口再多一点缓冲
    let max_win = *DEFAULT_WINDOWS.iter().max().unwrap_or(&20);
    let need = (max_win as usize) + 10;
    let bars = market::fetch_daily_voted(&code, need.max(60)).await;
    tracing::debug!(code, bars_len = bars.len(), "perf daily fetched");

    let (r1, r3, r5, r10, r20, pending) = if bars.len() < 2 || price <= 0.0 {
        (
            existing.0,
            existing.1,
            existing.2,
            existing.3,
            existing.4,
            DEFAULT_WINDOWS.to_vec(),
        )
    } else {
        let anchor = match locate_anchor(&bars, &fired_at) {
            Some(i) => i,
            None => {
                // 日线没覆盖到锚点，全部保留旧值
                return Ok(PerfRow {
                    signal_id: signal_id.into(),
                    code,
                    level,
                    fired_at,
                    price,
                    ret_1d: existing.0,
                    ret_3d: existing.1,
                    ret_5d: existing.2,
                    ret_10d: existing.3,
                    ret_20d: existing.4,
                    labeled_at: existing.5,
                    pending: DEFAULT_WINDOWS.to_vec(),
                });
            }
        };
        let mut pending = Vec::<i64>::new();
        let mut pick = |w: i64| -> Option<f64> {
            let idx = anchor + w as usize;
            if idx < bars.len() && bars[idx].close > 0.0 {
                Some(bars[idx].close / price - 1.0)
            } else {
                pending.push(w);
                None
            }
        };
        tracing::debug!(
            code,
            anchor,
            bars_len = bars.len(),
            price,
            "perf anchor located"
        );
        (pick(1), pick(3), pick(5), pick(10), pick(20), pending)
    };

    let labeled_at = if pending.is_empty() || (r5.is_some() && r10.is_some() && r20.is_some()) {
        Some(Utc::now().to_rfc3339())
    } else {
        existing.5.clone()
    };

    // upsert
    state.with_conn(|c| {
        c.execute(
            "INSERT INTO signal_performance(signal_id, ret_1d, ret_3d, ret_5d, ret_10d, ret_20d, labeled_at)
             VALUES(?,?,?,?,?,?,?)
             ON CONFLICT(signal_id) DO UPDATE SET
               ret_1d=excluded.ret_1d,
               ret_3d=excluded.ret_3d,
               ret_5d=excluded.ret_5d,
               ret_10d=excluded.ret_10d,
               ret_20d=excluded.ret_20d,
               labeled_at=excluded.labeled_at",
            params![signal_id, r1, r3, r5, r10, r20, labeled_at],
        )?;
        Ok(())
    })?;

    Ok(PerfRow {
        signal_id: signal_id.into(),
        code,
        level,
        fired_at,
        price,
        ret_1d: r1,
        ret_3d: r3,
        ret_5d: r5,
        ret_10d: r10,
        ret_20d: r20,
        labeled_at,
        pending,
    })
}

/// 扫描全部 signal，对**还没有 labeled_at 或窗口不全**的，尝试回填。
/// 返回 (total, updated, skipped)。
pub async fn backfill_all(
    state: &AppState,
    limit: usize,
) -> Result<(usize, usize, usize), AppError> {
    let ids: Vec<String> = state.with_conn(|c| {
        let mut stmt = c.prepare(
            "SELECT s.id FROM signal s
             LEFT JOIN signal_performance p ON p.signal_id = s.id
             WHERE s.price > 0
               AND (p.signal_id IS NULL
                    OR p.labeled_at IS NULL
                    OR p.ret_20d IS NULL)
             ORDER BY s.fired_at DESC
             LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;

    let total = ids.len();
    let mut updated = 0usize;
    let mut skipped = 0usize;
    for id in ids {
        match compute_returns(state, &id).await {
            Ok(row) => {
                // 至少一个窗口拿到了 ret 就算"updated"；
                // pending 为空 = 5/10/20 全部覆盖 = "fully updated"。
                if row.ret_5d.is_some()
                    || row.ret_10d.is_some()
                    || row.ret_20d.is_some()
                    || row.ret_1d.is_some()
                    || row.ret_3d.is_some()
                {
                    updated += 1;
                } else {
                    skipped += 1;
                }
            }
            Err(e) => {
                tracing::warn!(signal_id = %id, err = %e, "backfill skip");
                skipped += 1;
            }
        }
    }
    Ok((total, updated, skipped))
}

/// 计算 hit_rate：在给定窗口下，**正向**收益占比。仅统计 `ret_N IS NOT NULL` 的样本。
#[derive(Debug, Clone, Serialize)]
pub struct PerfStats {
    pub window: i64,
    pub samples: i64,
    pub hit_rate: Option<f64>,
    pub avg_return: Option<f64>,
    pub best: Option<f64>,
    pub worst: Option<f64>,
}

pub fn aggregate(state: &AppState, days: i64, limit: i64) -> Result<PerfStats, AppError> {
    let col = match days {
        1 => "ret_1d",
        3 => "ret_3d",
        5 => "ret_5d",
        10 => "ret_10d",
        20 => "ret_20d",
        _ => "ret_5d",
    };
    // SQLite 不支持在 LIMIT 中包聚合；改用子查询取最近 N 条，再聚合
    let sql = format!(
        "SELECT
            SUM(CASE WHEN x.{col} > 0 THEN 1 ELSE 0 END) AS hits,
            AVG(x.{col})                              AS avg_r,
            MAX(x.{col})                              AS best_r,
            MIN(x.{col})                              AS worst_r,
            COUNT(x.{col})                            AS n
         FROM (
           SELECT p.{col}
             FROM signal_performance p
             JOIN signal s ON s.id = p.signal_id
            WHERE p.{col} IS NOT NULL
              AND s.level IN ('confirm','strong')
            ORDER BY p.labeled_at DESC
            LIMIT ?
         ) x",
    );
    state.with_conn(|c| {
        let (hits, avg_r, best_r, worst_r, n): (
            Option<f64>,
            Option<f64>,
            Option<f64>,
            Option<f64>,
            i64,
        ) = c.query_row(&sql, params![limit], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        let hit_rate = if n > 0 {
            Some(hits.unwrap_or(0.0) / n as f64)
        } else {
            None
        };
        Ok(PerfStats {
            window: days,
            samples: n,
            hit_rate,
            avg_return: avg_r,
            best: best_r,
            worst: worst_r,
        })
    })
}

// （保留此位置作为扩展点；optional 查询走 `match` 内联处理）
