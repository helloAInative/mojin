//! 模拟撮合引擎（T+1 / 涨跌停 / 费率 / 零股 / 资金风控）。
//!
//! 纯本地纸上交易：**不接券商、不扣真实资金**。
//!
//! 设计要点：
//! - 订单提交立即锁资金/持仓（`pending`），并按当日行情立即撮合为 `filled`
//!   （本地模拟盘立即成交；买入股份在下一个自然日才计入 `available`）。
//! - A 股涨跌停：主板 10%；创业板/科创板 20%（按代码前缀判断）。
//! - 费率：佣金双边、最低 5 元；印花税仅卖出、千一；过户费双边、万分之 0.1。

use chrono::{FixedOffset, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::AppState;
use crate::error::AppError;
use crate::market::{self, Quote};

/// 涨跌停板幅度：主板 10%，创业板/科创板 20%。
pub fn price_limit_pct(code: &str) -> f64 {
    // 创业板 sz30x / sz00x 已经合并主板 10%；这里按代码前缀粗判：
    //  - sh688 科创板 20%
    //  - sz300 创业板 20%
    //  - sz/bj 8xx 北交所 30%（保守先按 30%）
    //  - 其他主板 10%
    if code.starts_with("sh688") || code.starts_with("sz300") {
        0.20
    } else if code.starts_with("bj") {
        0.30
    } else {
        0.10
    }
}

/// 限价上下沿。
pub fn price_limit(code: &str, prev_close: f64) -> (f64, f64) {
    let pct = price_limit_pct(code);
    if prev_close <= 0.0 {
        return (0.0, f64::MAX);
    }
    (prev_close * (1.0 - pct), prev_close * (1.0 + pct))
}

/// 费率计算（不含最低佣金的预扣，调用方按"双边，最低 5"应用）。
pub fn calc_fees(side: &str, qty: f64, price: f64) -> (f64, f64, f64) {
    let gross = qty * price;
    let raw_comm = gross * 0.00025;
    let commission = if raw_comm < 5.0 { 5.0 } else { raw_comm };
    let stamp_tax = if side == "sell" { gross * 0.001 } else { 0.0 };
    let transfer_fee = gross * 0.00001;
    (commission, stamp_tax, transfer_fee)
}

fn valid_quantity(side: &str, qty: f64) -> bool {
    qty.is_finite()
        && qty > 0.0
        && qty.fract() == 0.0
        && (side != "buy" || (qty >= 100.0 && qty % 100.0 == 0.0))
}

fn trading_day_start_utc() -> String {
    let china = FixedOffset::east_opt(8 * 3600).expect("valid UTC offset");
    Utc::now()
        .with_timezone(&china)
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("valid midnight")
        .and_local_timezone(china)
        .single()
        .expect("fixed offset is unambiguous")
        .with_timezone(&Utc)
        .to_rfc3339()
}

/// Reconcile persisted balances and sellable shares from the order/fill ledger.
fn sync_account_at(c: &Connection, account_id: &str, day_start: &str) -> Result<(), AppError> {
    c.execute(
        "UPDATE paper_position SET available = MAX(0, qty
            - COALESCE((SELECT SUM(f.qty) FROM paper_fill f
                WHERE f.account_id = paper_position.account_id AND f.code = paper_position.code
                  AND f.side = 'buy' AND f.filled_at >= ?2), 0)
            - COALESCE((SELECT SUM(o.qty) FROM paper_order o
                WHERE o.account_id = paper_position.account_id AND o.code = paper_position.code
                  AND o.side = 'sell' AND o.status = 'pending'), 0))
          WHERE account_id = ?1",
        params![account_id, day_start],
    )?;
    let mut pending = c.prepare(
        "SELECT side, qty, price FROM paper_order WHERE account_id = ? AND status = 'pending' AND side = 'buy'",
    )?;
    let mut frozen = 0.0;
    for row in pending.query_map(params![account_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, f64>(1)?,
            r.get::<_, f64>(2)?,
        ))
    })? {
        let (side, qty, price) = row?;
        let (commission, stamp_tax, transfer_fee) = calc_fees(&side, qty, price);
        frozen += qty * price + commission + stamp_tax + transfer_fee;
    }
    let cash: f64 = c.query_row(
        "SELECT cash FROM paper_account WHERE id = ?",
        params![account_id],
        |r| r.get(0),
    )?;
    let market_value: f64 = c.query_row(
        "SELECT COALESCE(SUM(CASE WHEN m.price > 0 THEN p.qty * m.price ELSE p.cost END), 0)
           FROM paper_position p LEFT JOIN paper_mark m
             ON m.account_id = p.account_id AND m.code = p.code
          WHERE p.account_id = ?",
        params![account_id],
        |r| r.get(0),
    )?;
    c.execute(
        "UPDATE paper_account SET frozen = ?, equity = ?, updated_at = datetime('now') WHERE id = ?",
        params![frozen, cash + frozen + market_value, account_id],
    )?;
    Ok(())
}

fn sync_account(c: &Connection, account_id: &str) -> Result<(), AppError> {
    sync_account_at(c, account_id, &trading_day_start_utc())
}

pub fn reconcile_all(state: &AppState) -> Result<(), AppError> {
    state.with_conn(|c| {
        let mut stmt = c.prepare("SELECT id FROM paper_account")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for id in ids {
            sync_account(c, &id)?;
        }
        Ok(())
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderView {
    pub id: String,
    pub account_id: String,
    pub code: String,
    pub name: String,
    pub side: String,
    pub qty: f64,
    pub price: f64,
    pub status: String,
    pub reason: String,
    pub strategy_id: Option<String>,
    pub signal_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FillView {
    pub id: String,
    pub order_id: String,
    pub account_id: String,
    pub code: String,
    pub side: String,
    pub qty: f64,
    pub price: f64,
    pub commission: f64,
    pub stamp_tax: f64,
    pub transfer_fee: f64,
    pub filled_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionView {
    pub code: String,
    pub name: String,
    pub qty: f64,
    pub available: f64,
    pub cost: f64,
    pub avg_cost: f64,
    pub market_price: Option<f64>,
    pub market_value: f64,
    pub mark_source: Option<String>,
    pub quote_time: Option<String>,
    pub marked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub id: String,
    pub name: String,
    pub cash: f64,
    pub frozen: f64,
    pub equity: f64,
    pub market_value: f64,
    pub unpriced_positions: usize,
    pub positions: Vec<PositionView>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderRequest {
    pub account_id: Option<String>,
    pub code: String,
    pub side: String, // "buy" | "sell"
    pub qty: f64,
    pub price: Option<f64>, // 不传就用最新行情
    #[serde(default)]
    pub strategy_id: Option<String>,
    #[serde(default)]
    pub signal_id: Option<String>,
    #[serde(default)]
    pub risk_plan: Option<OrderRiskPlanRequest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderRiskPlanRequest {
    pub stop_price: f64,
    pub take_profit_price: f64,
    #[serde(default = "default_risk_budget_pct")]
    pub risk_budget_pct: f64,
    #[serde(default)]
    pub suggested_position_pct: f64,
    #[serde(default)]
    pub basis: String,
}

fn default_risk_budget_pct() -> f64 {
    0.01
}

#[derive(Debug, Clone, Serialize)]
pub struct OrderResp {
    pub ok: bool,
    pub order: OrderView,
    pub fill: Option<FillView>,
    pub account: AccountView,
    pub disclaimer: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskItem {
    pub code: String,
    pub name: String,
    pub qty: f64,
    pub available: f64,
    pub avg_cost: f64,
    pub market_price: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub stop_price: Option<f64>,
    pub take_profit_price: Option<f64>,
    pub distance_to_stop_pct: Option<f64>,
    pub distance_to_target_pct: Option<f64>,
    pub risk_budget_pct: Option<f64>,
    pub suggested_position_pct: Option<f64>,
    pub basis: Option<String>,
    pub source_order_id: Option<String>,
    pub status: &'static str,
    pub quote_source: Option<String>,
    pub quote_time: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskOverview {
    pub items: Vec<RiskItem>,
    pub total: usize,
    pub triggered: usize,
    pub near_stop: usize,
    pub unplanned: usize,
}

fn validate_risk_plan(side: &str, plan: &OrderRiskPlanRequest) -> Result<(), AppError> {
    if side != "buy" {
        return Err(AppError::Msg(
            "risk_plan is only supported for buy orders".into(),
        ));
    }
    if !plan.stop_price.is_finite()
        || !plan.take_profit_price.is_finite()
        || plan.stop_price <= 0.0
        || plan.take_profit_price <= plan.stop_price
    {
        return Err(AppError::Msg(
            "risk plan prices must be finite and take_profit_price > stop_price > 0".into(),
        ));
    }
    if !plan.risk_budget_pct.is_finite()
        || !(0.0..=1.0).contains(&plan.risk_budget_pct)
        || !plan.suggested_position_pct.is_finite()
        || !(0.0..=1.0).contains(&plan.suggested_position_pct)
    {
        return Err(AppError::Msg(
            "risk plan percentages must be finite values between 0 and 1".into(),
        ));
    }
    Ok(())
}

fn risk_status(price: Option<f64>, stop: Option<f64>, target: Option<f64>) -> &'static str {
    let (Some(price), Some(stop), Some(target)) = (price, stop, target) else {
        return "unplanned";
    };
    if price <= stop {
        "stop_triggered"
    } else if price >= target {
        "target_reached"
    } else if price <= stop * 1.02 {
        "near_stop"
    } else {
        "normal"
    }
}

/// 提交订单 → 立即锁资金/持仓 → 当日立即撮合。
pub async fn submit(state: &AppState, req: OrderRequest) -> Result<OrderResp, AppError> {
    // 1) 校验 side
    if req.side != "buy" && req.side != "sell" {
        return Err(AppError::Msg(format!("invalid side: {}", req.side)));
    }
    if !valid_quantity(&req.side, req.qty) {
        return Err(AppError::Msg(
            "qty must be a positive whole share count; buys must be multiples of 100".into(),
        ));
    }
    if let Some(plan) = &req.risk_plan {
        validate_risk_plan(&req.side, plan)?;
    }
    let account_id = req
        .account_id
        .clone()
        .unwrap_or_else(|| "default".to_string());
    if req
        .price
        .is_some_and(|price| !price.is_finite() || price <= 0.0)
    {
        return Err(AppError::Msg("price must be finite and > 0".into()));
    }

    // 2) 决定成交价：优先用请求 price，否则拉行情主源
    let mut price = req.price.unwrap_or(0.0);
    let mut name = String::new();
    let mut prev_close = 0.0;
    if price <= 0.0 {
        let q = pick_primary(&req.code).await;
        match q {
            Some(qq) => {
                price = qq.price;
                prev_close = qq.prev_close;
                name = qq.name;
            }
            None => return Err(AppError::Msg(format!("no quote for {}", req.code))),
        }
    } else {
        // 仍需 prev_close 做涨跌停校验
        if let Some(qq) = pick_primary(&req.code).await {
            prev_close = qq.prev_close;
            if name.is_empty() {
                name = qq.name;
            }
        }
    }
    if !price.is_finite() || price <= 0.0 {
        return Err(AppError::Msg("price must be > 0".into()));
    }
    if let Some(plan) = &req.risk_plan {
        if plan.stop_price >= price || plan.take_profit_price <= price {
            return Err(AppError::Msg(
                "risk plan must satisfy stop_price < order price < take_profit_price".into(),
            ));
        }
    }
    // 3) 涨跌停校验
    let (lo, hi) = price_limit(&req.code, prev_close);
    if prev_close > 0.0 {
        let over_hi = price > hi + 1e-6;
        let under_lo = price < lo - 1e-6;
        if over_hi || under_lo {
            let order_id = new_pending_order(
                state,
                &account_id,
                &req,
                price,
                "rejected",
                &format!("price {} outside limit [{}, {}]", price, lo, hi),
            )?;
            let account = load_account(state, &account_id)?;
            return Ok(OrderResp {
                ok: false,
                order: order_view(state, &order_id)?,
                fill: None,
                account,
                disclaimer: "模拟撮合 · 拒单 · 不构成投资建议",
            });
        }
    }
    // 4) 资金 / 持仓 风控 + 冻结
    let (commission, stamp_tax, transfer_fee) = calc_fees(&req.side, req.qty, price);
    let total_cost = req.qty * price + commission + transfer_fee + stamp_tax;
    if !total_cost.is_finite() {
        return Err(AppError::Msg("order amount is too large".into()));
    }
    let (order_id, can_proceed) = state.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        sync_account(&tx, &account_id)?;
        // 先校验，再记录订单，避免把正在校验的卖单计入冻结持仓。
        let id = Uuid::new_v4().to_string();
        let mut can = true;
        let mut reject_reason = String::new();
        if req.side == "buy" {
            let cash: f64 = tx.query_row(
                "SELECT cash FROM paper_account WHERE id = ?",
                params![account_id],
                |r| r.get(0),
            )?;
            if cash + 1e-6 < total_cost {
                can = false;
                reject_reason = format!("insufficient cash: need {:.2}, have {:.2}", total_cost, cash);
            } else {
                tx.execute(
                    "UPDATE paper_account SET cash = cash - ?, updated_at = datetime('now')
                     WHERE id = ?",
                    params![total_cost, account_id],
                )?;
            }
        } else {
            let available: f64 = tx.query_row(
                "SELECT COALESCE(available,0) FROM paper_position WHERE account_id = ? AND code = ?",
                params![account_id, req.code],
                |r| r.get(0),
            ).optional()?.unwrap_or(0.0);
            if available + 1e-6 < req.qty {
                can = false;
                reject_reason = format!("insufficient available: need {}, have {}", req.qty, available);
            }
        }
        tx.execute(
            "INSERT INTO paper_order(id, account_id, code, side, qty, price, status, reason, strategy_id, signal_id)
             VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![
                id, account_id, req.code, req.side, req.qty, price,
                if can { "pending" } else { "rejected" }, reject_reason,
                req.strategy_id, req.signal_id
            ],
        )?;
        if can {
            if let Some(plan) = &req.risk_plan {
                tx.execute(
                    "INSERT INTO paper_risk_plan(
                       account_id, code, source_order_id, entry_price, stop_price,
                       take_profit_price, risk_budget_pct, suggested_position_pct, basis)
                     VALUES(?,?,?,?,?,?,?,?,?)
                     ON CONFLICT(account_id, code) DO UPDATE SET
                       source_order_id=excluded.source_order_id,
                       entry_price=excluded.entry_price,
                       stop_price=excluded.stop_price,
                       take_profit_price=excluded.take_profit_price,
                       risk_budget_pct=excluded.risk_budget_pct,
                       suggested_position_pct=excluded.suggested_position_pct,
                       basis=excluded.basis,
                       updated_at=datetime('now')",
                    params![
                        account_id, req.code, id, price, plan.stop_price,
                        plan.take_profit_price, plan.risk_budget_pct,
                        plan.suggested_position_pct, plan.basis
                    ],
                )?;
            }
        }
        sync_account(&tx, &account_id)?;
        tx.commit()?;
        Ok((id, can))
    })?;

    if !can_proceed {
        let account = load_account(state, &account_id)?;
        return Ok(OrderResp {
            ok: false,
            order: order_view(state, &order_id)?,
            fill: None,
            account,
            disclaimer: "模拟撮合 · 拒单 · 不构成投资建议",
        });
    }

    // 5) 立即撮合为 filled；新买入的股份到下一个自然日才可卖出。
    let fill = fill_order(
        state,
        &order_id,
        price,
        &name,
        commission,
        stamp_tax,
        transfer_fee,
    )?;

    let account = load_account(state, &account_id)?;
    Ok(OrderResp {
        ok: true,
        order: order_view(state, &order_id)?,
        fill: Some(fill),
        account,
        disclaimer: "模拟撮合 · 纸上成交 · 不构成投资建议",
    })
}

pub async fn risk_overview(state: &AppState, account_id: &str) -> Result<RiskOverview, AppError> {
    type RiskRow = (
        String,
        String,
        f64,
        f64,
        f64,
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<f64>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<RiskRow> = state.with_conn(|c| {
        let mut stmt = c.prepare(
            "SELECT p.code, p.name, p.qty, p.available, p.cost,
                    m.price, m.source, m.quote_time,
                    r.stop_price, r.take_profit_price, r.risk_budget_pct,
                    r.suggested_position_pct, r.basis, r.source_order_id
               FROM paper_position p
               LEFT JOIN paper_mark m ON m.account_id=p.account_id AND m.code=p.code
               LEFT JOIN paper_risk_plan r ON r.account_id=p.account_id AND r.code=p.code
              WHERE p.account_id=? AND p.qty > 0 ORDER BY p.code",
        )?;
        let rows = stmt
            .query_map(params![account_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                    r.get(12)?,
                    r.get(13)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })?;

    let mut items = Vec::with_capacity(rows.len());
    for (
        code,
        name,
        qty,
        available,
        cost,
        marked_price,
        marked_source,
        marked_time,
        stop_price,
        take_profit_price,
        risk_budget_pct,
        suggested_position_pct,
        basis,
        source_order_id,
    ) in rows
    {
        let quote = pick_primary(&code).await;
        let market_price = quote.as_ref().map(|q| q.price).or(marked_price);
        let quote_source = quote.as_ref().map(|q| q.source.clone()).or(marked_source);
        let quote_time = quote.as_ref().map(|q| q.time_text.clone()).or(marked_time);
        let avg_cost = if qty > 0.0 { cost / qty } else { 0.0 };
        let pnl_pct = market_price
            .filter(|_| avg_cost > 0.0)
            .map(|p| p / avg_cost - 1.0);
        let distance_to_stop_pct = match (market_price, stop_price) {
            (Some(price), Some(stop)) if price > 0.0 => Some((price - stop) / price),
            _ => None,
        };
        let distance_to_target_pct = match (market_price, take_profit_price) {
            (Some(price), Some(target)) if price > 0.0 => Some((target - price) / price),
            _ => None,
        };
        items.push(RiskItem {
            code,
            name,
            qty,
            available,
            avg_cost,
            market_price,
            pnl_pct,
            stop_price,
            take_profit_price,
            distance_to_stop_pct,
            distance_to_target_pct,
            risk_budget_pct,
            suggested_position_pct,
            basis,
            source_order_id,
            status: risk_status(market_price, stop_price, take_profit_price),
            quote_source,
            quote_time,
        });
    }
    let triggered = items
        .iter()
        .filter(|item| matches!(item.status, "stop_triggered" | "target_reached"))
        .count();
    let near_stop = items
        .iter()
        .filter(|item| item.status == "near_stop")
        .count();
    let unplanned = items
        .iter()
        .filter(|item| item.status == "unplanned")
        .count();
    Ok(RiskOverview {
        total: items.len(),
        items,
        triggered,
        near_stop,
        unplanned,
    })
}

fn new_pending_order(
    state: &AppState,
    account_id: &str,
    req: &OrderRequest,
    price: f64,
    status: &str,
    reason: &str,
) -> Result<String, AppError> {
    state.with_conn(|c| {
        let id = Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO paper_order(id, account_id, code, side, qty, price, status, reason, strategy_id, signal_id)
             VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![
                id, account_id, req.code, req.side, req.qty, price, status, reason,
                req.strategy_id, req.signal_id
            ],
        )?;
        Ok(id)
    })
}

pub fn fill_order(
    state: &AppState,
    order_id: &str,
    price: f64,
    name: &str,
    commission: f64,
    stamp_tax: f64,
    transfer_fee: f64,
) -> Result<FillView, AppError> {
    state.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        let (account_id, code, side, qty, status): (String, String, String, f64, String) = tx.query_row(
            "SELECT account_id, code, side, qty, status FROM paper_order WHERE id = ?",
            params![order_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        if status != "pending" {
            return Err(AppError::Msg(format!("order not pending: {status}")));
        }
        let gross = qty * price;
        // 1) 写 fill
        let fill_id = Uuid::new_v4().to_string();
        let filled_at = Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO paper_fill(id, order_id, account_id, code, side, qty, price, commission, stamp_tax, transfer_fee, slippage, filled_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                fill_id, order_id, account_id, code, side, qty, price,
                commission, stamp_tax, transfer_fee, 0.0, filled_at
            ],
        )?;
        // 2) 更新订单
        tx.execute(
            "UPDATE paper_order SET status='filled', reason='', updated_at=datetime('now') WHERE id = ?",
            params![order_id],
        )?;
        // 3) 更新账户 & 持仓
        let total_fees = commission + transfer_fee + stamp_tax;
        if side == "buy" {
            // 下单时已扣总成本；成交后按账本重算冻结资金和权益。
            tx.execute(
                "INSERT INTO paper_position(account_id, code, name, qty, available, cost)
                 VALUES(?,?,?,?,?,?)
                 ON CONFLICT(account_id, code) DO UPDATE SET
                   qty = qty + excluded.qty,
                   cost = cost + excluded.cost,
                   name = excluded.name",
                params![account_id, code, name, qty, 0.0, gross],
            )?;
        } else {
            // 卖出：现金 += gross - sell_fees。
            let net = gross - total_fees;
            tx.execute(
                "UPDATE paper_account SET cash = cash + ?, updated_at = datetime('now')
                 WHERE id = ?",
                params![net, account_id],
            )?;
            // 持仓：qty -=；available 由账本重算。
            tx.execute(
                "UPDATE paper_position SET qty = qty - ? WHERE account_id = ? AND code = ?",
                params![qty, account_id, code],
            )?;
            // 持仓 cost 按比例减
            let (cur_qty, cur_cost): (f64, f64) = tx.query_row(
                "SELECT qty, cost FROM paper_position WHERE account_id = ? AND code = ?",
                params![account_id, code],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if cur_qty > 0.0 {
                let new_cost = cur_cost * (cur_qty / (cur_qty + qty));
                tx.execute(
                    "UPDATE paper_position SET cost = ? WHERE account_id = ? AND code = ?",
                    params![new_cost, account_id, code],
                )?;
            } else {
                // 清仓
                tx.execute(
                    "DELETE FROM paper_position WHERE account_id = ? AND code = ?",
                    params![account_id, code],
                )?;
                tx.execute(
                    "DELETE FROM paper_mark WHERE account_id = ? AND code = ?",
                    params![account_id, code],
                )?;
                tx.execute(
                    "DELETE FROM paper_risk_plan WHERE account_id = ? AND code = ?",
                    params![account_id, code],
                )?;
            }
        }
        sync_account(&tx, &account_id)?;
        tx.commit()?;
        Ok(FillView {
            id: fill_id,
            order_id: order_id.into(),
            account_id,
            code,
            side,
            qty,
            price,
            commission,
            stamp_tax,
            transfer_fee,
            filled_at,
        })
    })
}

pub fn cancel_order(state: &AppState, order_id: &str) -> Result<OrderView, AppError> {
    state.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        let (account_id, side, qty, price, status): (String, String, f64, f64, String) = tx.query_row(
            "SELECT account_id, side, qty, price, status FROM paper_order WHERE id = ?",
            params![order_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        if status != "pending" {
            return Err(AppError::Msg(format!("order not pending: {status}")));
        }
        if side == "buy" {
            let (commission, stamp_tax, transfer_fee) = calc_fees(&side, qty, price);
            let total_cost = qty * price + commission + transfer_fee + stamp_tax;
            tx.execute(
                "UPDATE paper_account SET cash = cash + ?, updated_at = datetime('now')
                 WHERE id = ?",
                params![total_cost, account_id],
            )?;
        }
        tx.execute(
            "UPDATE paper_order SET status='cancelled', reason='user cancelled', updated_at=datetime('now')
             WHERE id = ?",
            params![order_id],
        )?;
        sync_account(&tx, &account_id)?;
        tx.commit()?;
        Ok(())
    })?;
    order_view(state, order_id)
}

pub fn order_view(state: &AppState, order_id: &str) -> Result<OrderView, AppError> {
    state.with_conn(|c| {
        c.query_row(
            "SELECT id, account_id, code, '', side, qty, price, status, reason,
                    strategy_id, signal_id, created_at, updated_at
               FROM paper_order WHERE id = ?",
            params![order_id],
            |r| {
                Ok(OrderView {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    code: r.get(2)?,
                    name: r.get(3)?,
                    side: r.get(4)?,
                    qty: r.get(5)?,
                    price: r.get(6)?,
                    status: r.get(7)?,
                    reason: r.get(8)?,
                    strategy_id: r.get(9)?,
                    signal_id: r.get(10)?,
                    created_at: r.get(11)?,
                    updated_at: r.get(12)?,
                })
            },
        )
        .map_err(AppError::Db)
    })
}

pub fn load_account(state: &AppState, account_id: &str) -> Result<AccountView, AppError> {
    state.with_conn(|c| {
        sync_account(c, account_id)?;
        let (id, name, cash, frozen, equity): (String, String, f64, f64, f64) = c.query_row(
            "SELECT id, name, cash, frozen, equity FROM paper_account WHERE id = ?",
            params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        let mut stmt = c.prepare(
            "SELECT p.code, p.name, p.qty, p.available, p.cost,
                    m.price, m.source, m.quote_time, m.fetched_at
               FROM paper_position p LEFT JOIN paper_mark m
                 ON m.account_id = p.account_id AND m.code = p.code
              WHERE p.account_id = ? ORDER BY p.code",
        )?;
        let positions = stmt
            .query_map(params![account_id], |r| {
                let qty: f64 = r.get(2)?;
                let cost: f64 = r.get(4)?;
                let market_price: Option<f64> = r.get(5)?;
                Ok(PositionView {
                    code: r.get(0)?,
                    name: r.get(1)?,
                    qty,
                    available: r.get(3)?,
                    cost,
                    avg_cost: if qty > 0.0 { cost / qty } else { 0.0 },
                    market_price,
                    market_value: market_price.map_or(cost, |price| qty * price),
                    mark_source: r.get(6)?,
                    quote_time: r.get(7)?,
                    marked_at: r.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let market_value: f64 = positions.iter().map(|p| p.market_value).sum();
        let unpriced_positions = positions
            .iter()
            .filter(|p| p.market_price.is_none())
            .count();
        Ok(AccountView {
            id,
            name,
            cash,
            frozen,
            equity,
            market_value,
            unpriced_positions,
            positions,
        })
    })
}

pub fn list_orders(
    state: &AppState,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<OrderView>, AppError> {
    state.with_conn(|c| {
        let (sql, has_filter) = match status {
            Some(_) => (
                "SELECT id, account_id, code, '', side, qty, price, status, reason,
                        strategy_id, signal_id, created_at, updated_at
                   FROM paper_order WHERE status = ? ORDER BY created_at DESC LIMIT ?",
                true,
            ),
            None => (
                "SELECT id, account_id, code, '', side, qty, price, status, reason,
                        strategy_id, signal_id, created_at, updated_at
                   FROM paper_order ORDER BY created_at DESC LIMIT ?",
                false,
            ),
        };
        let mut stmt = c.prepare(sql)?;
        let map_row = |r: &rusqlite::Row| -> rusqlite::Result<OrderView> {
            Ok(OrderView {
                id: r.get(0)?,
                account_id: r.get(1)?,
                code: r.get(2)?,
                name: r.get(3)?,
                side: r.get(4)?,
                qty: r.get(5)?,
                price: r.get(6)?,
                status: r.get(7)?,
                reason: r.get(8)?,
                strategy_id: r.get(9)?,
                signal_id: r.get(10)?,
                created_at: r.get(11)?,
                updated_at: r.get(12)?,
            })
        };
        let rows = if has_filter {
            stmt.query_map(params![status.unwrap(), limit], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![limit], map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    })
}

pub fn list_fills(state: &AppState, limit: i64) -> Result<Vec<FillView>, AppError> {
    state.with_conn(|c| {
        let mut stmt = c.prepare(
            "SELECT id, order_id, account_id, code, side, qty, price, commission, stamp_tax, transfer_fee, filled_at
               FROM paper_fill ORDER BY filled_at DESC LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(FillView {
                    id: r.get(0)?,
                    order_id: r.get(1)?,
                    account_id: r.get(2)?,
                    code: r.get(3)?,
                    side: r.get(4)?,
                    qty: r.get(5)?,
                    price: r.get(6)?,
                    commission: r.get(7)?,
                    stamp_tax: r.get(8)?,
                    transfer_fee: r.get(9)?,
                    filled_at: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

/// Refresh all held positions before committing a daily snapshot.
pub async fn snapshot(state: &AppState, account_id: &str) -> Result<AccountView, AppError> {
    let account = load_account(state, account_id)?;
    let codes: Vec<String> = account.positions.into_iter().map(|p| p.code).collect();
    let results =
        futures::future::join_all(codes.iter().map(|code| market::fetch_quote_ranked(code))).await;
    let mut quotes = Vec::with_capacity(codes.len());
    for (code, sources) in codes.iter().zip(results) {
        let quote = sources
            .into_iter()
            .find(|q| q.price.is_finite() && q.price > 0.0)
            .ok_or_else(|| AppError::Unavailable(format!("行情不可用，未写入快照: {code}")))?;
        quotes.push(quote);
    }
    let china = FixedOffset::east_opt(8 * 3600).expect("valid UTC offset");
    let as_of = Utc::now()
        .with_timezone(&china)
        .format("%Y-%m-%d")
        .to_string();
    snapshot_with_quotes(state, account_id, &as_of, &quotes)
}

fn snapshot_with_quotes(
    state: &AppState,
    account_id: &str,
    as_of: &str,
    quotes: &[Quote],
) -> Result<AccountView, AppError> {
    state.with_conn(|c| {
        let tx = c.unchecked_transaction()?;
        let mut stmt =
            tx.prepare("SELECT code FROM paper_position WHERE account_id = ? ORDER BY code")?;
        let codes = stmt
            .query_map(params![account_id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        let fetched_at = Utc::now().to_rfc3339();
        for code in &codes {
            let q = quotes
                .iter()
                .find(|q| q.code == *code && q.price.is_finite() && q.price > 0.0)
                .ok_or_else(|| AppError::Msg(format!("行情不完整，未写入快照: {code}")))?;
            tx.execute(
                "INSERT INTO paper_mark(account_id, code, price, source, quote_time, fetched_at)
                 VALUES(?,?,?,?,?,?)
                 ON CONFLICT(account_id, code) DO UPDATE SET
                   price=excluded.price, source=excluded.source,
                   quote_time=excluded.quote_time, fetched_at=excluded.fetched_at",
                params![account_id, code, q.price, q.source, q.time_text, fetched_at],
            )?;
        }
        sync_account(&tx, account_id)?;
        let (equity, cash): (f64, f64) = tx.query_row(
            "SELECT equity, cash FROM paper_account WHERE id = ?",
            params![account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let previous: Option<f64> = tx
            .query_row(
                "SELECT equity FROM paper_daily_pnl WHERE account_id = ? AND as_of < ?
             ORDER BY as_of DESC LIMIT 1",
                params![account_id, as_of],
                |r| r.get(0),
            )
            .optional()?;
        let pnl = previous.map_or(0.0, |value| equity - value);
        let ret = previous
            .filter(|value| *value > 0.0)
            .map_or(0.0, |value| pnl / value);
        tx.execute(
            "INSERT INTO paper_daily_pnl(account_id, as_of, equity, cash, pnl, ret, bench_ret)
             VALUES(?,?,?,?,?,?,NULL)
             ON CONFLICT(account_id, as_of) DO UPDATE SET
               equity=excluded.equity, cash=excluded.cash, pnl=excluded.pnl,
               ret=excluded.ret, bench_ret=excluded.bench_ret",
            params![account_id, as_of, equity, cash, pnl, ret],
        )?;
        tx.commit()?;
        Ok(())
    })?;
    load_account(state, account_id)
}

async fn pick_primary(code: &str) -> Option<Quote> {
    market::fetch_quote_ranked(code).await.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn state() -> AppState {
        let state = AppState::open(Path::new(":memory:")).unwrap();
        state.migrate().unwrap();
        state
    }

    fn pending_buy(state: &AppState, id: &str, qty: f64, price: f64) {
        let (commission, stamp, transfer) = calc_fees("buy", qty, price);
        let total = qty * price + commission + stamp + transfer;
        state
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO paper_order(id, account_id, code, side, qty, price, status)
                 VALUES(?1, 'default', 'sz000001', 'buy', ?2, ?3, 'pending')",
                    params![id, qty, price],
                )?;
                c.execute(
                    "UPDATE paper_account SET cash = cash - ? WHERE id = 'default'",
                    params![total],
                )?;
                sync_account(c, "default")?;
                Ok(())
            })
            .unwrap();
    }

    fn near(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.000001,
            "{actual} != {expected}"
        );
    }

    fn quote(price: f64) -> Quote {
        Quote {
            code: "sz000001".into(),
            name: "平安银行".into(),
            price,
            prev_close: price,
            open: price,
            high: price,
            low: price,
            change: 0.0,
            pct: 0.0,
            time_text: "2026-09-18 15:00".into(),
            source: "测试行情".into(),
        }
    }

    #[test]
    fn buy_lots_and_sell_shares_are_validated() {
        assert!(valid_quantity("buy", 100.0));
        assert!(valid_quantity("buy", 200.0));
        assert!(!valid_quantity("buy", 101.0));
        assert!(!valid_quantity("buy", 0.0));
        assert!(!valid_quantity("buy", f64::NAN));
        assert!(valid_quantity("sell", 1.0));
        assert!(!valid_quantity("sell", 1.5));
    }

    #[test]
    fn risk_plan_validation_and_status_are_deterministic() {
        let valid = OrderRiskPlanRequest {
            stop_price: 9.0,
            take_profit_price: 12.0,
            risk_budget_pct: 0.01,
            suggested_position_pct: 0.15,
            basis: String::new(),
        };
        assert!(validate_risk_plan("buy", &valid).is_ok());
        assert!(validate_risk_plan("sell", &valid).is_err());
        assert_eq!(
            risk_status(Some(8.9), Some(9.0), Some(12.0)),
            "stop_triggered"
        );
        assert_eq!(risk_status(Some(9.1), Some(9.0), Some(12.0)), "near_stop");
        assert_eq!(
            risk_status(Some(12.0), Some(9.0), Some(12.0)),
            "target_reached"
        );
        assert_eq!(risk_status(Some(10.0), Some(9.0), Some(12.0)), "normal");
        assert_eq!(risk_status(Some(10.0), None, None), "unplanned");
    }

    #[test]
    fn buy_fill_reconciles_cash_equity_and_t_plus_one() {
        let state = state();
        pending_buy(&state, "buy-1", 100.0, 10.0);
        let pending = load_account(&state, "default").unwrap();
        near(pending.cash, 98_994.99);
        near(pending.frozen, 1_005.01);
        near(pending.equity, 100_000.0);

        let fill = fill_order(&state, "buy-1", 10.0, "平安银行", 5.0, 0.0, 0.01).unwrap();
        assert_eq!(fill.order_id, "buy-1");
        let account = load_account(&state, "default").unwrap();
        near(account.cash, 98_994.99);
        near(account.frozen, 0.0);
        near(account.equity, 99_994.99);
        near(account.positions[0].available, 0.0);
        near(account.positions[0].qty, 100.0);
        assert!(fill_order(&state, "buy-1", 10.0, "平安银行", 5.0, 0.0, 0.01).is_err());

        state
            .with_conn(|c| {
                sync_account_at(c, "default", "9999-01-01T00:00:00+00:00")?;
                let available: f64 = c.query_row(
                    "SELECT available FROM paper_position WHERE account_id = 'default'",
                    [],
                    |r| r.get(0),
                )?;
                near(available, 100.0);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn sell_fill_and_pending_cancellation_keep_balances_consistent() {
        let state = state();
        pending_buy(&state, "buy-1", 100.0, 10.0);
        fill_order(&state, "buy-1", 10.0, "平安银行", 5.0, 0.0, 0.01).unwrap();
        state
            .with_conn(|c| {
                c.execute(
                    "UPDATE paper_fill SET filled_at = '2000-01-01T00:00:00+00:00'",
                    [],
                )?;
                c.execute(
                    "INSERT INTO paper_risk_plan(
                       account_id, code, source_order_id, entry_price, stop_price, take_profit_price)
                     VALUES('default', 'sz000001', 'buy-1', 10, 9, 12)",
                    [],
                )?;
                sync_account(c, "default")?;
                c.execute(
                    "INSERT INTO paper_order(id, account_id, code, side, qty, price, status)
                 VALUES('sell-1', 'default', 'sz000001', 'sell', 100, 12, 'pending')",
                    [],
                )?;
                sync_account(c, "default")?;
                Ok(())
            })
            .unwrap();
        near(
            load_account(&state, "default").unwrap().positions[0].available,
            0.0,
        );
        let cancelled = cancel_order(&state, "sell-1").unwrap();
        assert_eq!(cancelled.status, "cancelled");
        near(
            load_account(&state, "default").unwrap().positions[0].available,
            100.0,
        );

        state
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO paper_order(id, account_id, code, side, qty, price, status)
                 VALUES('sell-2', 'default', 'sz000001', 'sell', 100, 12, 'pending')",
                    [],
                )?;
                sync_account(c, "default")?;
                Ok(())
            })
            .unwrap();
        fill_order(&state, "sell-2", 12.0, "平安银行", 5.0, 1.2, 0.012).unwrap();
        let account = load_account(&state, "default").unwrap();
        near(account.cash, 100_188.778);
        near(account.equity, account.cash);
        near(account.frozen, 0.0);
        assert!(account.positions.is_empty());
        let risk_plans: i64 = state
            .with_conn(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM paper_risk_plan", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(risk_plans, 0);
    }

    #[test]
    fn cancelling_pending_buy_restores_cash_without_deadlock() {
        let state = state();
        pending_buy(&state, "buy-1", 100.0, 10.0);
        assert_eq!(cancel_order(&state, "buy-1").unwrap().status, "cancelled");
        let account = load_account(&state, "default").unwrap();
        near(account.cash, 100_000.0);
        near(account.frozen, 0.0);
        near(account.equity, 100_000.0);
    }

    #[test]
    fn snapshot_marks_holdings_and_uses_previous_day_equity() {
        let state = state();
        snapshot_with_quotes(&state, "default", "2026-09-17", &[]).unwrap();
        pending_buy(&state, "buy-1", 100.0, 10.0);
        fill_order(&state, "buy-1", 10.0, "平安银行", 5.0, 0.0, 0.01).unwrap();

        let account =
            snapshot_with_quotes(&state, "default", "2026-09-18", &[quote(12.0)]).unwrap();
        near(account.market_value, 1_200.0);
        near(account.equity, 100_194.99);
        assert_eq!(account.unpriced_positions, 0);
        assert_eq!(
            account.positions[0].mark_source.as_deref(),
            Some("测试行情")
        );

        snapshot_with_quotes(&state, "default", "2026-09-18", &[quote(13.0)]).unwrap();
        let (equity, pnl, ret): (f64, f64, f64) = state
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT equity, pnl, ret FROM paper_daily_pnl WHERE as_of = '2026-09-18'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .unwrap();
        near(equity, 100_294.99);
        near(pnl, 294.99);
        near(ret, 294.99 / 100_000.0);

        assert!(snapshot_with_quotes(&state, "default", "2026-09-19", &[]).is_err());
        let count: i64 = state
            .with_conn(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM paper_daily_pnl", [], |r| r.get(0))?)
            })
            .unwrap();
        assert_eq!(count, 2);
        near(
            load_account(&state, "default").unwrap().market_value,
            1_300.0,
        );
    }

    #[test]
    fn incomplete_snapshot_rolls_back_marks_for_all_positions() {
        let state = state();
        state
            .with_conn(|c| {
                c.execute(
                    "INSERT INTO paper_position(account_id, code, qty, available, cost)
                 VALUES('default', 'sz000001', 100, 100, 1000),
                       ('default', 'sz000002', 100, 100, 1000)",
                    [],
                )?;
                c.execute(
                    "UPDATE paper_account SET cash = 98000 WHERE id = 'default'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();

        assert!(snapshot_with_quotes(&state, "default", "2026-09-18", &[quote(12.0)]).is_err());
        let (marks, reports): (i64, i64) = state
            .with_conn(|c| {
                Ok((
                    c.query_row("SELECT COUNT(*) FROM paper_mark", [], |r| r.get(0))?,
                    c.query_row("SELECT COUNT(*) FROM paper_daily_pnl", [], |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!((marks, reports), (0, 0));
    }
}
