use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::ai;
use crate::db::AppState;
use crate::error::AppError;
use crate::export;
use crate::market::{self, Quote};
use crate::paper;
use crate::perf;
use crate::research;
use crate::signals::{self, Signal};
use crate::universe;
use rusqlite::{params, OptionalExtension};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/meta", get(meta))
        .route("/api/v1/paper/account", get(paper_account))
        .route("/api/v1/paper/positions", get(paper_positions))
        .route(
            "/api/v1/paper/orders",
            get(paper_orders).post(paper_submit_order),
        )
        .route(
            "/api/v1/paper/order/{id}",
            axum::routing::delete(paper_cancel_order),
        )
        .route("/api/v1/paper/fills", get(paper_fills))
        .route("/api/v1/paper/pnl", get(paper_pnl))
        .route("/api/v1/paper/snapshot", post(paper_snapshot))
        .route("/api/v1/quote/{code}", get(quote))
        .route("/api/v1/daily/{code}", get(daily))
        .route("/api/v1/signal/evaluate/{code}", post(evaluate_signal))
        .route("/api/v1/signals", get(list_signals))
        .route("/api/v1/scan", post(any_scan))
        // 信号后验
        .route("/api/v1/signal/{id}", get(signal_detail))
        .route("/api/v1/signal/{id}/backfill", post(signal_backfill))
        .route("/api/v1/signals/backfill", post(signals_backfill))
        .route("/api/v1/performance", get(performance))
        // 自选 / 指数池
        .route("/api/v1/watchlist", get(watchlist_list).post(watchlist_add))
        .route(
            "/api/v1/watchlist/item",
            axum::routing::delete(watchlist_del),
        )
        .route("/api/v1/watchlist/groups", get(watchlist_groups))
        .route("/api/v1/index-universe", get(index_list).post(index_add))
        .route(
            "/api/v1/index-universe/{code}",
            axum::routing::delete(index_del),
        )
        // AI 五模型并行
        .route("/api/v1/ai/analyze/{code}", post(ai_analyze))
        .route("/api/v1/ai/config", get(ai_config))
        .route("/api/v1/ai/usage", get(ai_usage))
        .route("/api/v1/research/context", get(research_context))
        .route("/api/v1/research/select", post(research_select))
        // WS 状态
        .route("/api/v1/ws/stats", get(ws_stats))
        // 报表导出（CSV / UTF-8 BOM，Excel/Numbers 直接打开）
        .route("/api/v1/export/signals.csv", get(export::export_signals))
        .route("/api/v1/export/fills.csv", get(export::export_fills))
        .route("/api/v1/export/pnl.csv", get(export::export_pnl))
        .route(
            "/api/v1/export/positions.csv",
            get(export::export_positions),
        )
}

#[derive(Serialize)]
struct Healthz {
    ok: bool,
    service: &'static str,
    product: &'static str,
    version: String,
    db_path: String,
    tables: i64,
    paper_equity: f64,
    signals: i64,
    signals_labeled: i64,
    hit_rate_5d: Option<f64>,
    hit_rate_10d: Option<f64>,
    hit_rate_20d: Option<f64>,
    ts: String,
    disclaimer: &'static str,
}

async fn healthz(State(state): State<Arc<AppState>>) -> Result<Json<Healthz>, AppError> {
    let version = state.schema_version().unwrap_or_else(|_| "unknown".into());
    let tables = state.table_count()?;
    let paper_equity = state.paper_equity().unwrap_or(0.0);
    let signals: i64 = state
        .with_conn(|c| {
            c.query_row("SELECT COUNT(*) FROM signal", [], |r| r.get(0))
                .map_err(AppError::Db)
        })
        .unwrap_or(0);
    let (signals_labeled, hit_rate_5d, hit_rate_10d, hit_rate_20d) = state
        .with_conn(|c| {
            let labeled: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM signal_performance WHERE labeled_at IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let hr = |col: &str| -> Option<f64> {
                let (hits, n): (f64, i64) = c
                    .query_row(
                        &format!(
                            "SELECT COALESCE(SUM(CASE WHEN {col} > 0 THEN 1 ELSE 0 END),0),
                                    COUNT({col})
                               FROM signal_performance p
                               JOIN signal s ON s.id = p.signal_id
                              WHERE p.{col} IS NOT NULL
                                AND s.level IN ('confirm','strong')"
                        ),
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .ok()?;
                if n == 0 {
                    None
                } else {
                    Some(hits / n as f64)
                }
            };
            Ok((labeled, hr("ret_5d"), hr("ret_10d"), hr("ret_20d")))
        })
        .unwrap_or((0, None, None, None));
    Ok(Json(Healthz {
        ok: true,
        service: "mj-server",
        product: "摸金小王子",
        version,
        db_path: state.db_path.display().to_string(),
        tables,
        paper_equity,
        signals,
        signals_labeled,
        hit_rate_5d,
        hit_rate_10d,
        hit_rate_20d,
        ts: Utc::now().to_rfc3339(),
        disclaimer: "家庭自用 · 纸上统计 · 不构成投资建议",
    }))
}

#[derive(Serialize)]
struct Meta {
    ok: bool,
    api: &'static str,
    endpoints: [&'static str; 30],
}

async fn meta() -> Json<Meta> {
    Json(Meta {
        ok: true,
        api: "v1",
        endpoints: [
            "/healthz",
            "/api/v1/meta",
            "/api/v1/paper/account",
            "/api/v1/paper/positions",
            "/api/v1/paper/orders",
            "/api/v1/paper/order/{id}",
            "/api/v1/paper/fills",
            "/api/v1/paper/pnl",
            "/api/v1/paper/snapshot",
            "/api/v1/quote/{code}",
            "/api/v1/daily/{code}",
            "/api/v1/signal/evaluate/{code}",
            "/api/v1/signals",
            "/api/v1/scan",
            "/api/v1/signal/{id}",
            "/api/v1/signal/{id}/backfill",
            "/api/v1/signals/backfill",
            "/api/v1/performance",
            "/api/v1/watchlist",
            "/api/v1/watchlist/item",
            "/api/v1/watchlist/groups",
            "/api/v1/index-universe",
            "/api/v1/index-universe/{code}",
            "/api/v1/ai/analyze/{code}",
            "/api/v1/ai/config",
            "/api/v1/ai/usage",
            "/api/v1/research/context",
            "/api/v1/research/select",
            "/api/v1/ws/stats",
            "/ws",
        ],
    })
}

#[derive(Serialize)]
struct PaperAccountWrap {
    ok: bool,
    #[serde(flatten)]
    account: paper::AccountView,
    disclaimer: &'static str,
}

async fn paper_account(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PaperAccountWrap>, AppError> {
    let account = paper::load_account(&state, "default")?;
    Ok(Json(PaperAccountWrap {
        ok: true,
        account,
        disclaimer: "纸上交易 · 不构成投资建议",
    }))
}

#[derive(Serialize)]
struct QuoteResp {
    ok: bool,
    primary: Option<Quote>,
    sources: Vec<Quote>,
    disclaimer: &'static str,
}

async fn quote(Path(code): Path<String>) -> Result<Json<QuoteResp>, AppError> {
    let quotes = market::fetch_quote_ranked(&code).await;
    let primary = quotes.first().cloned();
    Ok(Json(QuoteResp {
        ok: true,
        primary,
        sources: quotes,
        disclaimer: "家庭自用 · 行情快照 · 不构成投资建议",
    }))
}

#[derive(Serialize)]
struct DailyResp {
    ok: bool,
    bars: Vec<market::DayBar>,
    count: usize,
    disclaimer: &'static str,
}

#[derive(Deserialize, Default)]
struct DailyQuery {
    #[serde(default)]
    limit: Option<usize>,
}

async fn daily(
    Path(code): Path<String>,
    Query(q): Query<DailyQuery>,
) -> Result<Json<DailyResp>, AppError> {
    let limit = q.limit.unwrap_or(120);
    let bars = market::fetch_daily_voted(&code, limit).await;
    let count = bars.len();
    Ok(Json(DailyResp {
        ok: true,
        count,
        bars,
        disclaimer: "日线来自东财/腾讯历史；不构成投资建议",
    }))
}

#[derive(Serialize)]
struct SignalResp {
    ok: bool,
    signal: Option<Signal>,
    bars_count: usize,
    disclaimer: &'static str,
}

async fn evaluate_signal(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> Result<Json<SignalResp>, AppError> {
    let bars = market::fetch_daily_voted(&code, 120).await;
    let name = bars.first().map(|_| code.clone()).unwrap_or_default();
    let sig = signals::evaluate(&code, &name, &bars);
    let stored = sig.clone();
    if let Some(s) = stored {
        let id_for_backfill = s.id.clone();
        let _ = persist_signal(&state, &s);
        // WS 广播：signal 事件
        let evt_payload = serde_json::to_value(&s)
            .unwrap_or_else(|_| serde_json::json!({"id": s.id, "code": s.code}));
        let st = state.clone();
        tokio::spawn(async move {
            st.ws
                .publish(crate::ws::WsEvent::Signal {
                    ts: chrono::Utc::now().to_rfc3339(),
                    payload: evt_payload,
                })
                .await;
        });
        // 异步回填后验（不阻塞响应；20 日后才会有完整 ret_20d）
        let st = state.clone();
        tokio::spawn(async move {
            if let Err(e) = perf::compute_returns(&st, &id_for_backfill).await {
                tracing::warn!(signal_id = %id_for_backfill, err = %e, "perf backfill (eval) failed");
            }
        });
    }
    Ok(Json(SignalResp {
        ok: true,
        signal: sig,
        bars_count: bars.len(),
        disclaimer: "技术信号输出；不构成投资建议",
    }))
}

#[derive(Serialize)]
struct SignalRow {
    id: String,
    code: String,
    name: String,
    level: String,
    confidence: f64,
    title: String,
    price: f64,
    fired_at: String,
}

#[derive(Serialize)]
struct SignalListResp {
    ok: bool,
    signals: Vec<SignalRow>,
    count: i64,
    disclaimer: &'static str,
}

async fn list_signals(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SignalListResp>, AppError> {
    state
        .with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, code, name, level, confidence, title, price, fired_at
             FROM signal ORDER BY fired_at DESC LIMIT 50",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(SignalRow {
                        id: r.get(0)?,
                        code: r.get(1)?,
                        name: r.get(2)?,
                        level: r.get(3)?,
                        confidence: r.get(4)?,
                        title: r.get(5)?,
                        price: r.get(6)?,
                        fired_at: r.get(7)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let count: i64 = c.query_row("SELECT COUNT(*) FROM signal", [], |r| r.get(0))?;
            Ok(SignalListResp {
                ok: true,
                signals: rows,
                count,
                disclaimer: "历史信号；不构成投资建议",
            })
        })
        .map(Json)
}

#[derive(Deserialize, Default)]
struct ScanBody {
    codes: Vec<String>,
}

#[derive(Serialize)]
struct ScanResp {
    ok: bool,
    evaluated: usize,
    evaluated_bars: usize,
    triggered: usize,
    disclaimer: &'static str,
}

async fn any_scan(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ScanBody>,
) -> Result<Json<ScanResp>, AppError> {
    let codes: Vec<String> = if body.codes.is_empty() {
        // 默认看盘样本
        vec![
            "sz300623".into(), // 捷捷微电
            "sh600519".into(), // 贵州茅台
            "sz000001".into(), // 平安银行
            "sz399006".into(), // 创业板指
        ]
    } else {
        body.codes
    };
    let mut evaluated = 0usize;
    let mut evaluated_bars = 0usize;
    let mut triggered = 0usize;
    for code in codes {
        evaluated += 1;
        let bars = market::fetch_daily_voted(&code, 120).await;
        let name = code.clone();
        if !bars.is_empty() {
            evaluated_bars += 1;
            if let Some(sig) = signals::evaluate(&code, &name, &bars) {
                triggered += 1;
                let id_for_backfill = sig.id.clone();
                let _ = persist_signal(&state, &sig);
                let evt_payload = serde_json::to_value(&sig)
                    .unwrap_or_else(|_| serde_json::json!({"id": sig.id, "code": sig.code}));
                let st = state.clone();
                tokio::spawn(async move {
                    st.ws
                        .publish(crate::ws::WsEvent::Signal {
                            ts: chrono::Utc::now().to_rfc3339(),
                            payload: evt_payload,
                        })
                        .await;
                });
                let st = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = perf::compute_returns(&st, &id_for_backfill).await {
                        tracing::warn!(signal_id = %id_for_backfill, err = %e, "perf backfill (scan) failed");
                    }
                });
            }
        } else {
            tracing::warn!(code, "daily fetch returned empty");
        }
    }
    Ok(Json(ScanResp {
        ok: true,
        evaluated,
        evaluated_bars,
        triggered,
        disclaimer: "全市场扫描入口；不构成投资建议",
    }))
}

fn persist_signal(state: &Arc<AppState>, sig: &Signal) -> Result<(), AppError> {
    state.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO signal(id, code, name, level, confidence, title, body, period, price, fired_at, meta_json)
             VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            rusqlite::params![
                sig.id,
                sig.code,
                sig.name,
                format!("{:?}", sig.level).to_lowercase(),
                sig.confidence,
                sig.title,
                sig.body,
                sig.period,
                sig.price,
                sig.fired_at,
                serde_json::to_string(&sig.factors).unwrap_or_else(|_| "[]".into()),
            ],
        )?;
        for f in &sig.factors {
            tx.execute(
                "INSERT INTO signal_factor_hit(signal_id, factor_key, factor_value, weight, detail)
                 VALUES(?,?,?,?,?)",
                rusqlite::params![sig.id, f.key, f.value, f.weight, f.detail],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

// ──────────────── 信号后验 ────────────────

#[derive(Serialize)]
struct SignalDetailResp {
    ok: bool,
    signal: SignalRow,
    perf: Option<perf::PerfRow>,
    factors: Vec<signals::FactorHit>,
    disclaimer: &'static str,
}

async fn signal_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SignalDetailResp>, AppError> {
    state.with_conn(|c| {
        let row = c.query_row(
            "SELECT id, code, name, level, confidence, title, price, fired_at
               FROM signal WHERE id = ?",
            rusqlite::params![id],
            |r| {
                Ok(SignalRow {
                    id: r.get(0)?,
                    code: r.get(1)?,
                    name: r.get(2)?,
                    level: r.get(3)?,
                    confidence: r.get(4)?,
                    title: r.get(5)?,
                    price: r.get(6)?,
                    fired_at: r.get(7)?,
                })
            },
        );
        let signal = match row {
            Ok(s) => s,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                return Err(AppError::Msg(format!("signal {id} not found")))
            }
            Err(e) => return Err(AppError::Db(e)),
        };
        let perf = c
            .query_row(
                "SELECT p.signal_id, s.code, s.level, s.fired_at, s.price,
                        p.ret_1d, p.ret_3d, p.ret_5d, p.ret_10d, p.ret_20d, p.labeled_at
                   FROM signal_performance p
                   JOIN signal s ON s.id = p.signal_id
                  WHERE p.signal_id = ?",
                rusqlite::params![id],
                |r| {
                    let mut pending = Vec::<i64>::new();
                    let mut take = |idx: usize, w: i64| -> Option<f64> {
                        let v: Option<f64> = r.get(idx).ok().flatten();
                        if v.is_none() {
                            pending.push(w);
                        }
                        v
                    };
                    Ok(perf::PerfRow {
                        signal_id: r.get(0)?,
                        code: r.get(1)?,
                        level: r.get(2)?,
                        fired_at: r.get(3)?,
                        price: r.get(4)?,
                        ret_1d: take(5, 1),
                        ret_3d: take(6, 3),
                        ret_5d: take(7, 5),
                        ret_10d: take(8, 10),
                        ret_20d: take(9, 20),
                        labeled_at: r.get(10)?,
                        pending,
                    })
                },
            )
            .ok();
        let factors: Vec<signals::FactorHit> = {
            let mut stmt = c
                .prepare("SELECT factor_key, factor_value, weight, detail FROM signal_factor_hit WHERE signal_id = ? ORDER BY rowid")?;
            let rows = stmt
                .query_map(rusqlite::params![id], |r| {
                    Ok(signals::FactorHit {
                        key: r.get(0)?,
                        value: r.get(1)?,
                        weight: r.get(2)?,
                        detail: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        Ok(Json(SignalDetailResp {
            ok: true,
            signal,
            perf,
            factors,
            disclaimer: "家庭自用 · 不构成投资建议",
        }))
    })
}

#[derive(Serialize)]
struct BackfillResp {
    ok: bool,
    perf: perf::PerfRow,
    disclaimer: &'static str,
}

async fn signal_backfill(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<BackfillResp>, AppError> {
    let perf = perf::compute_returns(&state, &id).await?;
    Ok(Json(BackfillResp {
        ok: true,
        perf,
        disclaimer: "信号后验为纸上统计；不构成投资建议",
    }))
}

#[derive(Deserialize, Default)]
struct BackfillBody {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
struct BackfillAllResp {
    ok: bool,
    total: usize,
    updated: usize,
    skipped: usize,
    disclaimer: &'static str,
}

async fn signals_backfill(
    State(state): State<Arc<AppState>>,
    body: Option<Json<BackfillBody>>,
) -> Result<Json<BackfillAllResp>, AppError> {
    let limit = body.and_then(|b| b.0.limit).unwrap_or(500);
    let (total, updated, skipped) = perf::backfill_all(&state, limit).await?;
    Ok(Json(BackfillAllResp {
        ok: true,
        total,
        updated,
        skipped,
        disclaimer: "批量回填 5/10/20 日后验；不构成投资建议",
    }))
}

#[derive(Deserialize, Default)]
struct PerfQuery {
    #[serde(default)]
    days: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Serialize)]
struct PerfResp {
    ok: bool,
    stats: perf::PerfStats,
    windows: Vec<perf::PerfStats>,
    disclaimer: &'static str,
}

async fn performance(
    State(state): State<Arc<AppState>>,
    Query(q): Query<PerfQuery>,
) -> Result<Json<PerfResp>, AppError> {
    let days = q.days.unwrap_or(5);
    let limit = q.limit.unwrap_or(500);
    let primary = perf::aggregate(&state, days, limit)?;
    let windows: Vec<perf::PerfStats> = [1, 3, 5, 10, 20]
        .iter()
        .filter_map(|&w| perf::aggregate(&state, w, limit).ok())
        .collect();
    Ok(Json(PerfResp {
        ok: true,
        stats: primary,
        windows,
        disclaimer: "仅供家庭自用 · 后验统计 · 不构成投资建议",
    }))
}

// ──────────────── 模拟撮合 ────────────────

#[derive(Serialize)]
struct PositionsResp {
    ok: bool,
    positions: Vec<paper::PositionView>,
    disclaimer: &'static str,
}

async fn paper_positions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PositionsResp>, AppError> {
    let account = paper::load_account(&state, "default")?;
    Ok(Json(PositionsResp {
        ok: true,
        positions: account.positions,
        disclaimer: "模拟盘持仓 · 不构成投资建议",
    }))
}

#[derive(Deserialize, Default)]
struct OrdersQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Serialize)]
struct OrdersResp {
    ok: bool,
    orders: Vec<paper::OrderView>,
    disclaimer: &'static str,
}

async fn paper_orders(
    State(state): State<Arc<AppState>>,
    Query(q): Query<OrdersQuery>,
) -> Result<Json<OrdersResp>, AppError> {
    let orders = paper::list_orders(&state, q.status.as_deref(), q.limit.unwrap_or(50))?;
    Ok(Json(OrdersResp {
        ok: true,
        orders,
        disclaimer: "模拟盘订单 · 不构成投资建议",
    }))
}

async fn paper_submit_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<paper::OrderRequest>,
) -> Result<Json<paper::OrderResp>, AppError> {
    let resp = paper::submit(&state, req).await?;
    if let Some(fill) = &resp.fill {
        let payload =
            serde_json::to_value(fill).unwrap_or_else(|_| serde_json::json!({"id": fill.id}));
        state
            .ws
            .publish(crate::ws::WsEvent::Fill {
                ts: chrono::Utc::now().to_rfc3339(),
                payload,
            })
            .await;
        // 同时推一份最新 equity
        let eq = state.paper_equity().unwrap_or(0.0);
        state
            .ws
            .publish(crate::ws::WsEvent::Equity {
                ts: chrono::Utc::now().to_rfc3339(),
                payload: serde_json::json!({ "equity": eq }),
            })
            .await;
    }
    Ok(Json(resp))
}

async fn paper_cancel_order(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<OrdersResp>, AppError> {
    let _ = paper::cancel_order(&state, &id)?;
    let orders = paper::list_orders(&state, None, 50)?;
    Ok(Json(OrdersResp {
        ok: true,
        orders,
        disclaimer: "模拟盘撤单 · 不构成投资建议",
    }))
}

#[derive(Deserialize, Default)]
struct FillsQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Serialize)]
struct FillsResp {
    ok: bool,
    fills: Vec<paper::FillView>,
    disclaimer: &'static str,
}

async fn paper_fills(
    State(state): State<Arc<AppState>>,
    Query(q): Query<FillsQuery>,
) -> Result<Json<FillsResp>, AppError> {
    let fills = paper::list_fills(&state, q.limit.unwrap_or(50))?;
    Ok(Json(FillsResp {
        ok: true,
        fills,
        disclaimer: "模拟盘成交 · 不构成投资建议",
    }))
}

#[derive(Deserialize, Default)]
struct PnlQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Serialize)]
struct PnlRow {
    as_of: String,
    equity: f64,
    cash: f64,
    pnl: f64,
    ret: f64,
    bench_ret: Option<f64>,
}

#[derive(Serialize)]
struct PnlResp {
    ok: bool,
    rows: Vec<PnlRow>,
    disclaimer: &'static str,
}

async fn paper_pnl(
    State(state): State<Arc<AppState>>,
    Query(q): Query<PnlQuery>,
) -> Result<Json<PnlResp>, AppError> {
    let limit = q.limit.unwrap_or(60);
    let rows = state.with_conn(|c| -> Result<Vec<PnlRow>, AppError> {
        let mut stmt = c.prepare(
            "SELECT as_of, equity, cash, pnl, ret, bench_ret FROM paper_daily_pnl
             WHERE account_id = 'default' ORDER BY as_of DESC LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(PnlRow {
                    as_of: r.get(0)?,
                    equity: r.get(1)?,
                    cash: r.get(2)?,
                    pnl: r.get(3)?,
                    ret: r.get(4)?,
                    bench_ret: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;
    Ok(Json(PnlResp {
        ok: true,
        rows,
        disclaimer: "模拟盘日终 PnL · 不构成投资建议",
    }))
}

async fn paper_snapshot(
    State(state): State<Arc<AppState>>,
) -> Result<Json<PaperAccountWrap>, AppError> {
    let account = paper::snapshot(&state, "default").await?;
    Ok(Json(PaperAccountWrap {
        ok: true,
        account,
        disclaimer: "持仓行情估值与当日快照已写入 · 不构成投资建议",
    }))
}

// ──────────────── 自选 / 指数池 ────────────────

#[derive(Deserialize, Default)]
struct WatchQuery {
    #[serde(default)]
    group: Option<String>,
}

#[derive(Serialize)]
struct WatchListResp {
    ok: bool,
    items: Vec<universe::WatchItem>,
    disclaimer: &'static str,
}

async fn watchlist_list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WatchQuery>,
) -> Result<Json<WatchListResp>, AppError> {
    let items = universe::list_watch(&state, q.group.as_deref())?;
    Ok(Json(WatchListResp {
        ok: true,
        items,
        disclaimer: "家庭自用 · 自选股清单",
    }))
}

#[derive(Serialize)]
struct WatchItemResp {
    ok: bool,
    item: universe::WatchItem,
    disclaimer: &'static str,
}

async fn watchlist_add(
    State(state): State<Arc<AppState>>,
    Json(req): Json<universe::WatchAdd>,
) -> Result<Json<WatchItemResp>, AppError> {
    let item = universe::add_watch(&state, req)?;
    Ok(Json(WatchItemResp {
        ok: true,
        item,
        disclaimer: "家庭自用 · 自选股已记录",
    }))
}

#[derive(Deserialize, Default)]
struct WatchDelQuery {
    #[serde(default)]
    group: Option<String>,
    pub code: String,
}

#[derive(Serialize)]
struct WatchDelResp {
    ok: bool,
    removed: usize,
    disclaimer: &'static str,
}

async fn watchlist_del(
    State(state): State<Arc<AppState>>,
    Query(q): Query<WatchDelQuery>,
) -> Result<Json<WatchDelResp>, AppError> {
    let g = q.group.unwrap_or_else(|| "默认".to_string());
    let n = universe::del_watch(&state, &g, &q.code)?;
    Ok(Json(WatchDelResp {
        ok: true,
        removed: n,
        disclaimer: "家庭自用 · 自选股已删除",
    }))
}

#[derive(Serialize)]
struct GroupsResp {
    ok: bool,
    groups: Vec<String>,
    disclaimer: &'static str,
}

async fn watchlist_groups(
    State(state): State<Arc<AppState>>,
) -> Result<Json<GroupsResp>, AppError> {
    let groups = universe::list_groups(&state)?;
    Ok(Json(GroupsResp {
        ok: true,
        groups,
        disclaimer: "家庭自用 · 自选股分组",
    }))
}

#[derive(Deserialize, Default)]
struct IndexQuery {
    #[serde(default)]
    enabled_only: Option<bool>,
}

#[derive(Serialize)]
struct IndexListResp {
    ok: bool,
    items: Vec<universe::IndexItem>,
    disclaimer: &'static str,
}

async fn index_list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<IndexQuery>,
) -> Result<Json<IndexListResp>, AppError> {
    let items = universe::list_index(&state, q.enabled_only.unwrap_or(false))?;
    Ok(Json(IndexListResp {
        ok: true,
        items,
        disclaimer: "家庭自用 · 指数池",
    }))
}

#[derive(Serialize)]
struct IndexItemResp {
    ok: bool,
    item: universe::IndexItem,
    disclaimer: &'static str,
}

async fn index_add(
    State(state): State<Arc<AppState>>,
    Json(req): Json<universe::IndexAdd>,
) -> Result<Json<IndexItemResp>, AppError> {
    let item = universe::add_index(&state, req)?;
    Ok(Json(IndexItemResp {
        ok: true,
        item,
        disclaimer: "家庭自用 · 指数池已记录",
    }))
}

#[derive(Serialize)]
struct IndexDelResp {
    ok: bool,
    removed: usize,
    disclaimer: &'static str,
}

async fn index_del(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> Result<Json<IndexDelResp>, AppError> {
    let n = universe::del_index(&state, &code)?;
    Ok(Json(IndexDelResp {
        ok: true,
        removed: n,
        disclaimer: "家庭自用 · 指数池已删除",
    }))
}

// ──────────────── AI 五模型并行 + 仲裁 ────────────────

#[derive(Deserialize, Default)]
struct AnalyzeBody {
    #[serde(default)]
    signal_id: Option<String>,
    #[serde(default)]
    include_research: Option<bool>,
}

#[derive(Serialize)]
struct AnalyzeWrap {
    ok: bool,
    #[serde(flatten)]
    inner: ai::ArbitrateResp,
}

async fn ai_analyze(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
    body: Option<Json<AnalyzeBody>>,
) -> Result<Json<AnalyzeWrap>, AppError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let signal_id = body.signal_id;
    let research = if body.include_research.unwrap_or(true) {
        research::collect().await
    } else {
        research::ResearchContext::default()
    };
    let mut ctx = if let Some(signal_id) = signal_id.as_deref() {
        ai::context_for_signal(&state, signal_id, &code)?
    } else {
        current_ai_context(&state, &code).await?
    };
    ctx.research = research;
    let providers = ai::configured_providers();
    let resp = ai::analyze(&state, ctx, providers).await?;
    let payload = serde_json::to_value(&resp)
        .unwrap_or_else(|_| serde_json::json!({ "code": code, "signal_id": signal_id }));
    state
        .ws
        .publish(crate::ws::WsEvent::Ai {
            ts: chrono::Utc::now().to_rfc3339(),
            payload,
        })
        .await;
    Ok(Json(AnalyzeWrap {
        ok: true,
        inner: resp,
    }))
}

async fn current_ai_context(state: &AppState, code: &str) -> Result<ai::AiContext, AppError> {
    let bars = market::fetch_daily_voted(code, 120).await;
    if bars.is_empty() {
        return Err(AppError::Unavailable(format!(
            "no daily bars available for {code}"
        )));
    }
    let primary_quote = market::fetch_quote_ranked(code).await.into_iter().next();
    let name = primary_quote
        .as_ref()
        .map(|quote| quote.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| code.to_string());
    let price = primary_quote
        .as_ref()
        .map(|quote| quote.price)
        .unwrap_or_else(|| bars.last().map(|bar| bar.close).unwrap_or(0.0));
    let signal = signals::evaluate(code, &name, &bars);
    let signal_level = signal
        .as_ref()
        .map(|signal| format!("{:?}", signal.level).to_lowercase())
        .unwrap_or_else(|| "tip".into());
    let signal_confidence = signal
        .as_ref()
        .map(|signal| signal.confidence)
        .unwrap_or(0.0);
    let factors = signal
        .as_ref()
        .map(|signal| {
            signal
                .factors
                .iter()
                .map(|factor| (factor.key.clone(), factor.value, factor.detail.clone()))
                .collect()
        })
        .unwrap_or_default();
    let (ret_5d, ret_20d) =
        state.with_conn(|conn| -> Result<(Option<f64>, Option<f64>), AppError> {
            let row = conn
                .query_row(
                    "SELECT ret_5d, ret_20d FROM signal_performance p
                     JOIN signal s ON s.id = p.signal_id
                     WHERE s.code = ?
                     ORDER BY p.labeled_at DESC NULLS LAST, s.fired_at DESC LIMIT 1",
                    params![code],
                    |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, Option<f64>>(1)?)),
                )
                .optional()?;
            Ok((row.and_then(|item| item.0), row.and_then(|item| item.1)))
        })?;
    Ok(ai::AiContext {
        signal_id: None,
        code: code.to_string(),
        name,
        price,
        signal_level,
        signal_confidence,
        factors,
        ret_5d,
        ret_20d,
        historical_evidence: ai::historical_evidence(state, code)?,
        research: research::ResearchContext::default(),
    })
}

#[derive(Serialize)]
struct AiConfigResp {
    ok: bool,
    #[serde(flatten)]
    config: ai::AiConfigView,
    disclaimer: &'static str,
}

async fn ai_config() -> Json<AiConfigResp> {
    Json(AiConfigResp {
        ok: true,
        config: ai::config_view(),
        disclaimer: "Token 只从服务端环境变量读取，接口不会返回密钥",
    })
}

#[derive(Serialize)]
struct ResearchContextResp {
    ok: bool,
    context: research::ResearchContext,
    disclaimer: &'static str,
}

async fn research_context() -> Json<ResearchContextResp> {
    Json(ResearchContextResp {
        ok: true,
        context: research::collect().await,
        disclaimer: "新闻、海外行情和板块数据可能延迟或缺失；不构成投资建议",
    })
}

#[derive(Deserialize, Default)]
struct ResearchSelectBody {
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    max_candidates: Option<usize>,
}

#[derive(Serialize)]
struct ResearchFailure {
    code: String,
    error: String,
}

#[derive(Serialize)]
struct ResearchSelectResp {
    ok: bool,
    as_of: String,
    ai: ai::AiConfigView,
    context: research::ResearchContext,
    candidates: Vec<ai::ArbitrateResp>,
    failures: Vec<ResearchFailure>,
    methodology: &'static str,
    disclaimer: &'static str,
}

async fn research_select(
    State(state): State<Arc<AppState>>,
    body: Option<Json<ResearchSelectBody>>,
) -> Result<Json<ResearchSelectResp>, AppError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let mut codes = body.codes;
    if codes.is_empty() {
        codes = universe::list_watch(&state, None)?
            .into_iter()
            .map(|item| item.code)
            .collect();
    }
    if codes.is_empty() {
        codes = vec!["sz300623".into(), "sh600519".into(), "sz000001".into()];
    }
    codes.sort();
    codes.dedup();
    codes.truncate(12);
    let max_candidates = body.max_candidates.unwrap_or(5).clamp(1, 10);
    let context = research::collect().await;
    let ai_config = ai::config_view();
    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    for code in codes {
        match current_ai_context(&state, &code).await {
            Ok(mut ai_context) => {
                ai_context.research = context.clone();
                match ai::analyze(&state, ai_context, ai::configured_providers()).await {
                    Ok(candidate) => candidates.push(candidate),
                    Err(error) => failures.push(ResearchFailure {
                        code,
                        error: error.to_string(),
                    }),
                }
            }
            Err(error) => failures.push(ResearchFailure {
                code,
                error: error.to_string(),
            }),
        }
    }
    candidates.sort_by(|left, right| {
        let left_rank = left.final_score * left.final_confidence;
        let right_rank = right.final_score * right.final_confidence;
        right_rank.total_cmp(&left_rank)
    });
    candidates.truncate(max_candidates);
    Ok(Json(ResearchSelectResp {
        ok: true,
        as_of: chrono::Utc::now().to_rfc3339(),
        ai: ai_config,
        context,
        candidates,
        failures,
        methodology: "自选池逐股计算技术信号与历史后验，叠加新闻、昨日美股和行业板块上下文，再由五角色独立分析并按历史权重仲裁。",
        disclaimer: "候选排序是研究线索，不是个性化买卖指令；数据可能延迟、缺失或错误。",
    }))
}

#[derive(Deserialize, Default)]
struct UsageQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Serialize)]
struct UsageResp {
    ok: bool,
    rows: Vec<ai::UsageRow>,
    disclaimer: &'static str,
}

async fn ai_usage(
    State(state): State<Arc<AppState>>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<UsageResp>, AppError> {
    let rows = ai::list_usage(&state, q.limit.unwrap_or(50))?;
    Ok(Json(UsageResp {
        ok: true,
        rows,
        disclaimer: "AI 用量统计 · 仅供家庭自用对账",
    }))
}

#[derive(Serialize)]
struct WsStatsResp {
    ok: bool,
    clients: usize,
    topics: Vec<String>,
    disclaimer: &'static str,
}

async fn ws_stats(State(state): State<Arc<AppState>>) -> Json<WsStatsResp> {
    let (clients, topics) = state.ws.stats().await;
    Json(WsStatsResp {
        ok: true,
        clients,
        topics,
        disclaimer: "WebSocket hub 当前状态 · 仅供家庭自用",
    })
}
