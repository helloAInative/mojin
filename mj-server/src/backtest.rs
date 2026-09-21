//! 基于历史日线的技术信号逐日重放。
//!
//! 只使用每个交易日当时可见的数据，确认/强信号在下一交易日开盘价进入，
//! 持有固定交易日后按收盘价退出；交易之间不重叠，结果扣除往返成本。

use serde::Serialize;

use crate::market::DayBar;
use crate::signals::{self, Level};

#[derive(Debug, Clone, Serialize)]
pub struct BacktestTrade {
    pub entry_date: String,
    pub exit_date: String,
    pub entry_price: f64,
    pub exit_price: f64,
    pub holding_days: usize,
    pub level: &'static str,
    pub confidence: f64,
    pub title: String,
    pub factors: Vec<String>,
    pub gross_return: f64,
    pub net_return: f64,
    pub equity_after: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestResult {
    pub ok: bool,
    pub code: String,
    pub period_start: String,
    pub period_end: String,
    pub bars: usize,
    pub scanned_days: usize,
    pub holding_days: usize,
    pub round_trip_cost_bps: f64,
    pub trades: Vec<BacktestTrade>,
    pub trade_count: usize,
    pub wins: usize,
    pub hit_rate: Option<f64>,
    pub avg_return: Option<f64>,
    pub total_return: f64,
    pub max_drawdown: f64,
    pub benchmark_return: f64,
    pub excess_return: f64,
    pub disclaimer: &'static str,
}

pub fn run(
    code: &str,
    bars: &[DayBar],
    holding_days: usize,
    round_trip_cost_bps: f64,
) -> BacktestResult {
    const WARMUP: usize = 30;
    let mut trades = Vec::new();
    let mut equity: f64 = 1.0;
    let mut peak: f64 = 1.0;
    let mut max_drawdown: f64 = 0.0;
    let mut scanned_days = 0;
    let cost = round_trip_cost_bps / 10_000.0;
    let mut index = WARMUP.saturating_sub(1);

    while index + holding_days < bars.len() {
        scanned_days += 1;
        let signal = signals::evaluate(code, code, &bars[..=index]);
        let eligible = signal
            .as_ref()
            .is_some_and(|item| matches!(item.level, Level::Confirm | Level::Strong));
        if !eligible {
            index += 1;
            continue;
        }

        let signal = signal.expect("eligible signal exists");
        let entry = &bars[index + 1];
        let exit = &bars[index + holding_days];
        if entry.open <= 0.0 || exit.close <= 0.0 {
            index += 1;
            continue;
        }
        let gross_return = exit.close / entry.open - 1.0;
        let net_return = gross_return - cost;
        equity *= (1.0 + net_return).max(0.0);
        peak = peak.max(equity);
        if peak > 0.0 {
            max_drawdown = max_drawdown.min(equity / peak - 1.0);
        }
        trades.push(BacktestTrade {
            entry_date: entry.date.clone(),
            exit_date: exit.date.clone(),
            entry_price: entry.open,
            exit_price: exit.close,
            holding_days,
            level: match signal.level {
                Level::Strong => "strong",
                Level::Confirm => "confirm",
                Level::Tip => "tip",
                Level::Risk => "risk",
            },
            confidence: signal.confidence,
            title: signal.title,
            factors: signal
                .factors
                .into_iter()
                .map(|factor| factor.key)
                .collect(),
            gross_return,
            net_return,
            equity_after: equity,
        });
        index += holding_days;
    }

    let trade_count = trades.len();
    let wins = trades.iter().filter(|trade| trade.net_return > 0.0).count();
    let hit_rate = (trade_count > 0).then_some(wins as f64 / trade_count as f64);
    let avg_return = (trade_count > 0)
        .then_some(trades.iter().map(|trade| trade.net_return).sum::<f64>() / trade_count as f64);
    let benchmark_return = if bars.len() > WARMUP && bars[WARMUP].open > 0.0 {
        bars.last()
            .map_or(0.0, |last| last.close / bars[WARMUP].open - 1.0)
    } else {
        0.0
    };
    let total_return = equity - 1.0;
    BacktestResult {
        ok: true,
        code: code.to_string(),
        period_start: bars
            .get(WARMUP)
            .or_else(|| bars.first())
            .map(|bar| bar.date.clone())
            .unwrap_or_default(),
        period_end: bars.last().map(|bar| bar.date.clone()).unwrap_or_default(),
        bars: bars.len(),
        scanned_days,
        holding_days,
        round_trip_cost_bps,
        trades,
        trade_count,
        wins,
        hit_rate,
        avg_return,
        total_return,
        max_drawdown,
        benchmark_return,
        excess_return: total_return - benchmark_return,
        disclaimer: "历史逐日重放 · 未考虑停牌与成交容量 · 不构成投资建议",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(index: usize, close: f64, volume: f64) -> DayBar {
        DayBar {
            code: "sz000001".into(),
            date: format!("2026-01-{:02}", index + 1),
            open: close,
            high: close * 1.01,
            low: close * 0.99,
            close,
            volume,
            amount: 0.0,
        }
    }

    #[test]
    fn backtest_never_looks_past_exit_and_keeps_metrics_finite() {
        let bars: Vec<DayBar> = (0..80)
            .map(|index| {
                let close = 10.0 + index as f64 * 0.08;
                let volume = if index % 8 == 0 { 3_000.0 } else { 1_000.0 };
                bar(index, close, volume)
            })
            .collect();
        let result = run("sz000001", &bars, 5, 10.0);
        assert_eq!(result.bars, 80);
        assert_eq!(result.holding_days, 5);
        assert!(result.total_return.is_finite());
        assert!(result.max_drawdown.is_finite());
        assert!(result
            .trades
            .iter()
            .all(|trade| trade.exit_date > trade.entry_date));
    }

    #[test]
    fn short_history_returns_an_empty_result() {
        let bars: Vec<DayBar> = (0..20).map(|index| bar(index, 10.0, 1_000.0)).collect();
        let result = run("sz000001", &bars, 5, 10.0);
        assert_eq!(result.trade_count, 0);
        assert_eq!(result.scanned_days, 0);
        assert_eq!(result.total_return, 0.0);
    }
}
