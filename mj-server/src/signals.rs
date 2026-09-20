//! 信号引擎初版：MACD / KDJ / RSI / 量能 → 信号 + 等级 + 置信

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::market::DayBar;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Tip,     // 提示
    Confirm, // 确认
    Strong,  // 强信号
    Risk,    // 风险预警
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub id: String,
    pub code: String,
    pub name: String,
    pub level: Level,
    pub confidence: f64,
    pub title: String,
    pub body: String,
    pub period: String,
    pub fired_at: String,
    pub factors: Vec<FactorHit>,
    pub why: String,
    pub price: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactorHit {
    pub key: String,
    pub value: f64,
    pub weight: f64,
    pub detail: String,
}

pub fn evaluate(code: &str, name: &str, bars: &[DayBar]) -> Option<Signal> {
    if bars.len() < 30 {
        return None;
    }
    let closes: Vec<f64> = bars.iter().map(|b| b.close).collect();
    let highs: Vec<f64> = bars.iter().map(|b| b.high).collect();
    let lows: Vec<f64> = bars.iter().map(|b| b.low).collect();
    let macd = compute_macd(&closes);
    let (k, d, j) = compute_kdj(&highs, &lows, &closes);
    let rsi = compute_rsi(&closes);
    let last = closes[closes.len() - 1];
    let mut hits: Vec<FactorHit> = Vec::new();
    let mut score = 0.0;
    let mut warns: Vec<String> = Vec::new();
    let mut title = String::new();

    // MACD 金叉 / 死叉
    if macd.len() >= 2 {
        let p = macd[macd.len() - 2];
        let c = macd[macd.len() - 1];
        if let (Some(p), Some(c)) = (p, c) {
            if p.0 <= p.1 && c.0 > c.1 {
                hits.push(FactorHit {
                    key: "macd_golden".into(),
                    value: c.0 - c.1,
                    weight: 1.4,
                    detail: format!("DIF {:.3} 上穿 DEA {:.3}", c.0, c.1),
                });
                score += 1.4;
                title = "MACD 金叉".into();
            } else if p.0 >= p.1 && c.0 < c.1 {
                hits.push(FactorHit {
                    key: "macd_death".into(),
                    value: c.0 - c.1,
                    weight: 1.4,
                    detail: format!("DIF {:.3} 下穿 DEA {:.3}", c.0, c.1),
                });
                score -= 1.4;
                title = "MACD 死叉".into();
            }
            if c.0 > c.1 {
                hits.push(FactorHit {
                    key: "macd_long".into(),
                    value: 1.0,
                    weight: 0.4,
                    detail: "DIF 在 DEA 之上，多头".into(),
                });
                score += 0.4;
            }
        }
    }

    // KDJ 超买 / 超卖
    if let (Some(jv), Some(kv), Some(_dv)) =
        (j.last().copied(), k.last().copied(), d.last().copied())
    {
        if jv > 100.0 || kv > 90.0 {
            warns.push(format!("KDJ 超买 J={:.0} K={:.0}", jv, kv));
            hits.push(FactorHit {
                key: "kdj_overbought".into(),
                value: jv,
                weight: 0.6,
                detail: "J/K 过高，警惕回踩".into(),
            });
            score -= 0.6;
        } else if jv < 0.0 || kv < 10.0 {
            hits.push(FactorHit {
                key: "kdj_oversold".into(),
                value: jv,
                weight: 0.6,
                detail: "J/K 过低，关注反弹".into(),
            });
            score += 0.6;
        }
    }

    // RSI 极值
    if let Some(r) = rsi.last().copied() {
        if let Some(rv) = r {
            if rv >= 80.0 {
                warns.push(format!("RSI={:.0}", rv));
                hits.push(FactorHit {
                    key: "rsi_high".into(),
                    value: rv,
                    weight: 0.4,
                    detail: "RSI 高位".into(),
                });
                score -= 0.4;
            } else if rv <= 20.0 {
                hits.push(FactorHit {
                    key: "rsi_low".into(),
                    value: rv,
                    weight: 0.4,
                    detail: "RSI 低位".into(),
                });
                score += 0.4;
            }
        }
    }

    // 量能：今日成交量较前 5 日均量
    if bars.len() >= 6 {
        let today_vol = bars[bars.len() - 1].volume;
        let avg5: f64 = bars[bars.len() - 6..bars.len() - 1]
            .iter()
            .map(|b| b.volume)
            .sum::<f64>()
            / 5.0;
        if avg5 > 0.0 {
            let ratio = today_vol / avg5;
            if ratio >= 2.0 {
                hits.push(FactorHit {
                    key: "vol_spike".into(),
                    value: ratio,
                    weight: 0.6,
                    detail: format!("放量 {:.1} 倍", ratio),
                });
                score += if ratio >= 3.0 { 0.9 } else { 0.6 };
                if title.is_empty() {
                    title = "放量异动".into();
                }
            } else if ratio <= 0.5 {
                hits.push(FactorHit {
                    key: "vol_shrink".into(),
                    value: ratio,
                    weight: 0.3,
                    detail: format!("缩量 {:.0}%", ratio * 100.0),
                });
            }
        }
    }

    if hits.is_empty() {
        return None;
    }

    let (level, conf) = classify(score, &warns);
    let fired_at = Utc::now().to_rfc3339();
    let body = format!(
        "{}{} | {}",
        title,
        if warns.is_empty() {
            String::new()
        } else {
            format!(" · {}", warns.join("·"))
        },
        bars.last().map(|b| b.date.clone()).unwrap_or_default()
    );
    let why = hits
        .iter()
        .map(|h| format!("{} {}", h.key, h.detail))
        .collect::<Vec<_>>()
        .join("；");
    Some(Signal {
        id: Uuid::new_v4().to_string(),
        code: code.into(),
        name: name.into(),
        level,
        confidence: conf,
        title: if title.is_empty() {
            "信号触发".into()
        } else {
            title
        },
        body,
        period: "1d".into(),
        fired_at,
        factors: hits,
        why,
        price: last,
    })
}

fn classify(score: f64, warns: &[String]) -> (Level, f64) {
    let score = score;
    let conf = (score.abs() / 3.0).min(1.0);
    if !warns.is_empty() && score <= 0.6 {
        (Level::Risk, conf.max(0.6))
    } else if score >= 2.4 {
        (Level::Strong, conf.max(0.6))
    } else if score >= 1.2 {
        (Level::Confirm, conf)
    } else if score > 0.0 {
        (Level::Tip, conf)
    } else {
        (Level::Risk, conf.max(0.5))
    }
}

/// EMA
fn ema(series: &[f64], period: usize) -> Vec<f64> {
    if series.is_empty() {
        return vec![];
    }
    let k = 2.0 / (period as f64 + 1.0);
    let mut out = Vec::with_capacity(series.len());
    let mut prev = series[0];
    out.push(prev);
    for v in &series[1..] {
        prev = *v * k + prev * (1.0 - k);
        out.push(prev);
    }
    out
}

fn compute_macd(closes: &[f64]) -> Vec<Option<(f64, f64, f64)>> {
    if closes.len() < 26 {
        return vec![None; closes.len()];
    }
    let ema12 = ema(closes, 12);
    let ema26 = ema(closes, 26);
    let mut dif = vec![0.0; closes.len()];
    for i in 0..closes.len() {
        dif[i] = ema12[i] - ema26[i];
    }
    let dea = ema(&dif, 9);
    closes
        .iter()
        .enumerate()
        .map(|(i, _)| Some((dif[i], dea[i], (dif[i] - dea[i]) * 2.0)))
        .collect()
}

fn compute_kdj(highs: &[f64], lows: &[f64], closes: &[f64]) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let n = closes.len();
    let mut k = vec![50.0; n];
    let mut d = vec![50.0; n];
    let mut j = vec![50.0; n];
    let period = 9;
    for i in 0..n {
        let start = i.saturating_sub(period - 1);
        let h_max = highs[start..=i].iter().cloned().fold(f64::MIN, f64::max);
        let l_min = lows[start..=i].iter().cloned().fold(f64::MAX, f64::min);
        let rsv = if (h_max - l_min).abs() < 1e-9 {
            50.0
        } else {
            (closes[i] - l_min) / (h_max - l_min) * 100.0
        };
        let prev_k = if i == 0 { 50.0 } else { k[i - 1] };
        let prev_d = if i == 0 { 50.0 } else { d[i - 1] };
        k[i] = prev_k * 2.0 / 3.0 + rsv / 3.0;
        d[i] = prev_d * 2.0 / 3.0 + k[i] / 3.0;
        j[i] = 3.0 * k[i] - 2.0 * d[i];
    }
    (k, d, j)
}

fn compute_rsi(closes: &[f64]) -> Vec<Option<f64>> {
    let period = 14;
    let n = closes.len();
    let mut out = vec![None; n];
    if n < period + 1 {
        return out;
    }
    let mut gain = 0.0;
    let mut loss = 0.0;
    for i in 1..=period {
        let diff = closes[i] - closes[i - 1];
        if diff > 0.0 {
            gain += diff;
        } else {
            loss -= diff;
        }
    }
    let mut avg_g = gain / period as f64;
    let mut avg_l = loss / period as f64;
    out[period] = if (avg_g + avg_l).abs() < 1e-9 {
        Some(50.0)
    } else {
        Some(100.0 - 100.0 / (1.0 + avg_g / avg_l))
    };
    for i in (period + 1)..n {
        let diff = closes[i] - closes[i - 1];
        let g = if diff > 0.0 { diff } else { 0.0 };
        let l = if diff < 0.0 { -diff } else { 0.0 };
        avg_g = (avg_g * (period as f64 - 1.0) + g) / period as f64;
        avg_l = (avg_l * (period as f64 - 1.0) + l) / period as f64;
        out[i] = if (avg_g + avg_l).abs() < 1e-9 {
            Some(50.0)
        } else {
            Some(100.0 - 100.0 / (1.0 + avg_g / avg_l))
        };
    }
    out
}
