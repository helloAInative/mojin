//! 报表导出：CSV（RFC 4180） + UTF-8 BOM，方便 Excel/Numbers 直接打开。
//!
//! 端点（`/api/v1/export/...`）：
//! - `signals.csv`    最近信号
//! - `fills.csv`      模拟成交
//! - `pnl.csv`        每日权益/盈亏快照
//! - `positions.csv`  当前持仓
//!
//! 设计：
//! - 不抽 trait、不引依赖；4 套表 4 个手写 writer，列和现有 API 对齐。
//! - 数字保持原始字符串，避免 Excel 自动把它当 % 截断。
//! - 字符集：UTF-8 + BOM，浏览器/Excel 中文不乱码；Numbers/WPS 直接识别。

use std::fmt::Write as _;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use rusqlite::params;
use serde::Deserialize;

use crate::db::AppState;
use crate::error::AppError;

const BOM: &str = "\u{feff}";

/// 转义一个 CSV 字段：包含 `,` / `"` / 换行 时整段用双引号包裹，内部 `"` 转成 `""`。
fn esc(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

/// `f64` 字段：保持原始字符串（不要 `%` / 截断）。None 输出空字符串。
fn num(v: Option<f64>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => String::new(),
    }
}

/// `f64` 字段固定小数位（资金/价格）。None 输出空字符串。
fn num_fixed(v: Option<f64>, digits: usize) -> String {
    match v {
        Some(x) => format!("{x:.*}", digits),
        None => String::new(),
    }
}

fn join_row(cells: &[String]) -> String {
    let mut s = String::new();
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(c);
    }
    s.push_str("\r\n"); // RFC 4180 用 CRLF
    s
}

fn csv_response(filename: &str, body: String) -> Response {
    let mut resp = (BOM.to_string() + &body).into_response();
    let h = resp.headers_mut();
    // 中文/空格文件名都安全：filename* 用 RFC 5987
    h.insert(
        header::CONTENT_TYPE,
        "text/csv; charset=utf-8".parse().unwrap(),
    );
    let disposition = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        filename,
        // percent-encode for filename*
        percent_encode(filename),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        disposition.parse().unwrap(),
    );
    resp
}

/// 最小百分比编码（只处理非 ASCII 和保留字符）。filename* 不需要严格 RFC 3986。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[derive(Deserialize, Default)]
pub struct LimitQuery {
    #[serde(default)]
    limit: Option<i64>,
}

fn clamp_limit(q: &LimitQuery, default: i64, cap: i64) -> i64 {
    q.limit.unwrap_or(default).clamp(1, cap)
}

// ──────────────── signals.csv ────────────────

pub async fn export_signals(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LimitQuery>,
) -> Result<Response, AppError> {
    let limit = clamp_limit(&q, 200, 5000);
    let mut body = String::new();
    body.push_str(&join_row(&[
        "fired_at".into(),
        "code".into(),
        "name".into(),
        "level".into(),
        "confidence".into(),
        "price".into(),
        "title".into(),
        "id".into(),
    ]));
    state.with_conn(|c| -> Result<(), AppError> {
        let mut stmt = c.prepare(
            "SELECT id, code, name, level, confidence, title, price, fired_at
             FROM signal ORDER BY fired_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok((
                r.get::<_, String>(7)?, // fired_at
                r.get::<_, String>(1)?, // code
                r.get::<_, String>(2)?, // name
                r.get::<_, String>(3)?, // level
                r.get::<_, f64>(4)?,    // confidence
                r.get::<_, f64>(6)?,    // price
                r.get::<_, String>(5)?, // title
                r.get::<_, String>(0)?, // id
            ))
        })?;
        for row in rows {
            let (fired_at, code, name, level, confidence, price, title, id) = row?;
            body.push_str(&join_row(&[
                esc(&fired_at),
                esc(&code),
                esc(&name),
                esc(&level),
                num(Some(confidence)),
                num_fixed(Some(price), 4),
                esc(&title),
                esc(&id),
            ]));
        }
        Ok(())
    })?;
    Ok(csv_response("mojin-signals.csv", body))
}

// ──────────────── fills.csv ────────────────

pub async fn export_fills(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LimitQuery>,
) -> Result<Response, AppError> {
    let limit = clamp_limit(&q, 200, 5000);
    let mut body = String::new();
    body.push_str(&join_row(&[
        "filled_at".into(),
        "code".into(),
        "side".into(),
        "qty".into(),
        "price".into(),
        "amount".into(),
        "commission".into(),
        "stamp_tax".into(),
        "transfer_fee".into(),
        "fee_total".into(),
        "order_id".into(),
    ]));
    let fills = crate::paper::list_fills(&state, limit)?;
    for f in fills {
        let amount = f.qty * f.price;
        let fee_total = f.commission + f.stamp_tax + f.transfer_fee;
        body.push_str(&join_row(&[
            esc(&f.filled_at),
            esc(&f.code),
            esc(&f.side),
            f.qty.to_string(),
            num_fixed(Some(f.price), 4),
            num_fixed(Some(amount), 2),
            num_fixed(Some(f.commission), 4),
            num_fixed(Some(f.stamp_tax), 4),
            num_fixed(Some(f.transfer_fee), 4),
            num_fixed(Some(fee_total), 4),
            esc(&f.order_id),
        ]));
    }
    Ok(csv_response("mojin-fills.csv", body))
}

// ──────────────── pnl.csv ────────────────

pub async fn export_pnl(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LimitQuery>,
) -> Result<Response, AppError> {
    let limit = clamp_limit(&q, 365, 5000);
    let mut body = String::new();
    body.push_str(&join_row(&[
        "as_of".into(),
        "equity".into(),
        "cash".into(),
        "pnl".into(),
        "ret".into(),
        "bench_ret".into(),
    ]));
    state.with_conn(|c| -> Result<(), AppError> {
        let mut stmt = c.prepare(
            "SELECT as_of, equity, cash, pnl, ret, bench_ret FROM paper_daily_pnl
             WHERE account_id = 'default' ORDER BY as_of DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, f64>(4)?,
                r.get::<_, Option<f64>>(5)?,
            ))
        })?;
        for row in rows {
            let (as_of, equity, cash, pnl, ret, bench_ret) = row?;
            body.push_str(&join_row(&[
                esc(&as_of),
                num_fixed(Some(equity), 2),
                num_fixed(Some(cash), 2),
                num_fixed(Some(pnl), 2),
                num_fixed(Some(ret), 6),
                num(bench_ret),
            ]));
        }
        Ok(())
    })?;
    Ok(csv_response("mojin-pnl.csv", body))
}

// ──────────────── positions.csv ────────────────

pub async fn export_positions(
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let account = crate::paper::load_account(&state, "default")?;
    let mut body = String::new();
    body.push_str(&join_row(&[
        "code".into(),
        "name".into(),
        "qty".into(),
        "available".into(),
        "cost".into(),
        "avg_cost".into(),
        "market_price".into(),
        "market_value".into(),
        "marked_at".into(),
        "mark_source".into(),
        "quote_time".into(),
    ]));
    for p in &account.positions {
        body.push_str(&join_row(&[
            esc(&p.code),
            esc(&p.name),
            p.qty.to_string(),
            p.available.to_string(),
            num_fixed(Some(p.cost), 2),
            num_fixed(Some(p.avg_cost), 4),
            num(p.market_price),
            num(Some(p.market_value)),
            esc(p.marked_at.as_deref().unwrap_or("")),
            esc(p.mark_source.as_deref().unwrap_or("")),
            esc(p.quote_time.as_deref().unwrap_or("")),
        ]));
    }
    Ok(csv_response("mojin-positions.csv", body))
}

// 抑制未用导入警告（Write trait 保留为后续扩展）
#[allow(dead_code)]
fn _fmt_write_keepalive() {
    let mut s = String::new();
    let _ = writeln!(s, "ok");
}