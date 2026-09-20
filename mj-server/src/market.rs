//! 行情采集：腾讯（主） / 东财（备） / 新浪（备）。
//! 复用现有桌面摸金里的解析思路，迁到 Rust。

use std::error::Error as _;
use std::time::Duration;

use serde::Serialize;

const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize)]
pub struct Quote {
    pub code: String,
    pub name: String,
    pub price: f64,
    pub prev_close: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub change: f64,
    pub pct: f64,
    pub time_text: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayBar {
    pub code: String,
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub amount: f64,
}

fn to_qq_prefix(code: &str) -> &'static str {
    if code.starts_with("sh") {
        "sh"
    } else if code.starts_with("bj") {
        "bj"
    } else {
        "sz"
    }
}

fn bare_code(code: &str) -> &str {
    if let Some(idx) = code.find(|c: char| !c.is_ascii_digit()) {
        if let Some(stripped) = code.get(idx..) {
            if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
                return stripped;
            }
        }
    }
    code
}

/// 腾讯 v_sh000001="~...~" 解析
pub async fn fetch_quote_tencent(code: &str) -> anyhow::Result<Quote> {
    let url = format!(
        "https://qt.gtimg.cn/q={}&_={}",
        code,
        chrono::Utc::now().timestamp_millis()
    );
    let body = http_get(&url, Some("https://stockapp.finance.qq.com/")).await?;
    let start = body
        .find('"')
        .ok_or_else(|| anyhow::anyhow!("tencent: no quote"))?
        + 1;
    let end = body
        .rfind('"')
        .ok_or_else(|| anyhow::anyhow!("tencent: no end"))?;
    let body = &body[start..end];
    let parts: Vec<&str> = body.split('~').collect();
    if parts.len() < 35 {
        anyhow::bail!("tencent: bad payload");
    }
    let price: f64 = parts[3].parse()?;
    let prev: f64 = parts[4].parse()?;
    let change: f64 = parts
        .get(31)
        .and_then(|s| s.parse().ok())
        .unwrap_or(price - prev);
    let pct: f64 = parts
        .get(32)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            if prev > 0.0 {
                change / prev * 100.0
            } else {
                0.0
            }
        });
    let high = parts.get(33).and_then(|s| s.parse().ok()).unwrap_or(price);
    let low = parts.get(34).and_then(|s| s.parse().ok()).unwrap_or(price);
    let time = parts.get(30).map(|s| s.to_string()).unwrap_or_default();
    Ok(Quote {
        code: code.to_string(),
        name: parts.get(1).map(|s| s.to_string()).unwrap_or_default(),
        price,
        prev_close: prev,
        open: parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(prev),
        high,
        low,
        change,
        pct,
        time_text: format_time(&time),
        source: "腾讯".into(),
    })
}

/// 东财 push2.eastmoney.com
pub async fn fetch_quote_eastmoney(code: &str) -> anyhow::Result<Quote> {
    let prefix = to_qq_prefix(code);
    let market = if prefix == "sh" { "1" } else { "0" };
    let bare = bare_code(code);
    let url = format!(
        "https://push2.eastmoney.com/api/qt/stock/get?secid={}.{}&fields=f43,f44,f45,f46,f57,f58,f60,f169,f170",
        market, bare
    );
    let body = http_get(&url, Some("https://quote.eastmoney.com/")).await?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let data = json
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("eastmoney: no data"))?;
    let scale =
        |k: &str| -> Option<f64> { data.get(k).and_then(|v| v.as_f64()).map(|v| v / 100.0) };
    let price = scale("f43").ok_or_else(|| anyhow::anyhow!("eastmoney: no price"))?;
    let prev = scale("f60").ok_or_else(|| anyhow::anyhow!("eastmoney: no prev"))?;
    let change = scale("f169").unwrap_or(price - prev);
    let pct = scale("f170").unwrap_or(if prev > 0.0 {
        change / prev * 100.0
    } else {
        0.0
    });
    let open = scale("f46").unwrap_or(prev);
    let high = scale("f44").unwrap_or(price);
    let low = scale("f45").unwrap_or(price);
    let name = data
        .get("f58")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    Ok(Quote {
        code: code.to_string(),
        name,
        price,
        prev_close: prev,
        open,
        high,
        low,
        change,
        pct,
        time_text: "--".into(),
        source: "东财".into(),
    })
}

/// 新浪 guba / hq.sinajs.cn
pub async fn fetch_quote_sina(code: &str) -> anyhow::Result<Quote> {
    let url = format!(
        "https://hq.sinajs.cn/list={}&_={}",
        code,
        chrono::Utc::now().timestamp_millis()
    );
    let body = http_get(&url, Some("https://finance.sina.com.cn/")).await?;
    let start = body
        .find('"')
        .ok_or_else(|| anyhow::anyhow!("sina: no quote"))?
        + 1;
    let end = body
        .rfind('"')
        .ok_or_else(|| anyhow::anyhow!("sina: no end"))?;
    let body = &body[start..end];
    let parts: Vec<&str> = body.split(',').collect();
    if parts.len() < 32 {
        anyhow::bail!("sina: bad payload");
    }
    let name = parts[0].to_string();
    let open: f64 = parts[1].parse()?;
    let prev: f64 = parts[2].parse()?;
    let price: f64 = parts[3].parse()?;
    let high: f64 = parts[4].parse()?;
    let low: f64 = parts[5].parse()?;
    let change = price - prev;
    let pct = if prev > 0.0 {
        change / prev * 100.0
    } else {
        0.0
    };
    let time = format!(
        "{} {}",
        parts.get(30).cloned().unwrap_or(""),
        parts.get(31).cloned().unwrap_or("")
    )
    .trim()
    .to_string();
    Ok(Quote {
        code: code.to_string(),
        name,
        price,
        prev_close: prev,
        open,
        high,
        low,
        change,
        pct,
        time_text: time,
        source: "新浪".into(),
    })
}

/// 按优先级获取多源行情：腾讯主，东财/新浪备。
pub async fn fetch_quote_ranked(code: &str) -> Vec<Quote> {
    let (a, b, c) = tokio::join!(
        fetch_quote_tencent(code),
        fetch_quote_eastmoney(code),
        fetch_quote_sina(code),
    );
    rank_valid_quotes([a.ok(), b.ok(), c.ok()])
}

fn rank_valid_quotes(sources: [Option<Quote>; 3]) -> Vec<Quote> {
    sources
        .into_iter()
        .flatten()
        .filter(|q| q.price.is_finite() && q.price > 0.0)
        .collect()
}

/// 腾讯日线 K 线（最近 ~120 根）
pub async fn fetch_daily_tencent(code: &str, limit: usize) -> anyhow::Result<Vec<DayBar>> {
    let url = format!(
        "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param={},day,,,{},qfq",
        code, limit
    );
    let body = http_get(&url, Some("https://stockapp.finance.qq.com/")).await?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let arr = json
        .get("data")
        .and_then(|d| d.get(code))
        .and_then(|c| c.get("qfqday"))
        .or_else(|| {
            json.get("data")
                .and_then(|d| d.get(code))
                .and_then(|c| c.get("day"))
        })
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("tencent daily: bad shape"))?;
    let mut bars = Vec::with_capacity(arr.len());
    for row in arr {
        let a = row
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("tencent daily: row"))?;
        if a.len() < 6 {
            continue;
        }
        let s = |i: usize| {
            a.get(i)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        let f = |i: usize| {
            a.get(i)
                .and_then(|v| {
                    v.as_str()
                        .and_then(|x| x.parse().ok())
                        .or_else(|| v.as_f64())
                })
                .unwrap_or(0.0)
        };
        // 腾讯 qfqday: [date, open, close, high, low, volume]
        bars.push(DayBar {
            code: code.to_string(),
            date: s(0),
            open: f(1),
            close: f(2),
            high: f(3),
            low: f(4),
            volume: f(5),
            amount: 0.0,
        });
    }
    Ok(bars)
}

/// 东财日线（备用） push2his.eastmoney.com
/// klt=101 前复权，fqt=1
pub async fn fetch_daily_eastmoney(code: &str, limit: usize) -> anyhow::Result<Vec<DayBar>> {
    let prefix = to_qq_prefix(code);
    let market = if prefix == "sh" {
        "1"
    } else if prefix == "bj" {
        "0"
    } else {
        "0"
    };
    let bare = bare_code(code);
    let url = format!(
        "https://push2his.eastmoney.com/api/qt/stock/kline/get?secid={}.{}&klt=101&fqt=1&lmt={}&fields1=f1,f2,f3,f4,f5&fields2=f51,f52,f53,f54,f55,f56,f57,f58",
        market, bare, limit
    );
    let body = http_get(&url, Some("https://quote.eastmoney.com/")).await?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let arr = json
        .get("data")
        .and_then(|d| d.get("klines"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("eastmoney daily: bad shape"))?;
    let mut bars = Vec::with_capacity(arr.len());
    for row in arr {
        let s = row
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("eastmoney daily: row"))?;
        // "2024-01-02,10.50,10.55,10.30,10.40,1000000,5000000,5.00,2.50,5.00"
        let p: Vec<&str> = s.split(',').collect();
        if p.len() < 6 {
            continue;
        }
        let parse = |i: usize| p.get(i).and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let parse_date = |i: usize| p.get(i).map(|x| x.to_string()).unwrap_or_default();
        bars.push(DayBar {
            code: code.to_string(),
            date: parse_date(0),
            open: parse(1),
            close: parse(2),
            high: parse(3),
            low: parse(4),
            volume: parse(5),
            amount: parse(6),
        });
    }
    Ok(bars)
}

/// 多源日线：腾讯主（稳定可用），东财备。
///
/// 历史注：东财 push2his 接口在部分出口 IP 下会返回 `rc=102`（限频/风控），
/// 腾讯 `web.ifzq.gtimg.cn/appstock/app/fqkline/get` 实测稳定且已是前复权，
/// 故把腾讯升为主；东财退为兜底，主源失败时尝试拿一份更全的（含 amount）。
pub async fn fetch_daily_voted(code: &str, limit: usize) -> Vec<DayBar> {
    // 1) 主：腾讯
    match fetch_daily_tencent(code, limit).await {
        Ok(b) if !b.is_empty() => {
            tracing::debug!(code, count = b.len(), "daily from tencent");
            return b;
        }
        Ok(_) => tracing::warn!(code, "tencent daily: empty"),
        Err(e) => tracing::warn!(code, err=%e, "tencent daily: err"),
    }
    // 2) 备：东财（限频时这里通常也会失败，但保留兜底）
    match fetch_daily_eastmoney(code, limit).await {
        Ok(b) if !b.is_empty() => {
            tracing::debug!(code, count = b.len(), "daily from eastmoney");
            return b;
        }
        Ok(_) => tracing::warn!(code, "eastmoney daily: empty"),
        Err(e) => tracing::warn!(code, err=%e, "eastmoney daily: err"),
    }
    Vec::new()
}

fn format_time(s: &str) -> String {
    if let Some(ts) = s.get(..14) {
        if ts.bytes().all(|b| b.is_ascii_digit()) {
            return format!(
                "{}-{}-{} {}:{}:{}",
                &ts[0..4],
                &ts[4..6],
                &ts[6..8],
                &ts[8..10],
                &ts[10..12],
                &ts[12..14]
            );
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::{format_time, rank_valid_quotes, Quote};

    fn quote(source: &str, price: f64) -> Quote {
        Quote {
            code: "sz000001".into(),
            name: "平安银行".into(),
            price,
            prev_close: 10.0,
            open: 10.0,
            high: 10.0,
            low: 10.0,
            change: 0.0,
            pct: 0.0,
            time_text: "2026-09-18 14:27:51".into(),
            source: source.into(),
        }
    }

    #[test]
    fn quote_time_keeps_calendar_date() {
        assert_eq!(format_time("20260918142751"), "2026-09-18 14:27:51");
        assert_eq!(format_time("--"), "--");
        assert_eq!(format_time("2026-09-18 14:27:51"), "2026-09-18 14:27:51");
    }

    #[test]
    fn fallback_prefers_eastmoney_when_tencent_is_invalid() {
        let ranked = rank_valid_quotes([
            Some(quote("腾讯", f64::INFINITY)),
            Some(quote("东财", 11.7)),
            Some(quote("新浪", 11.8)),
        ]);
        assert_eq!(ranked[0].source, "东财");
        assert_eq!(ranked[1].source, "新浪");
    }
}

async fn http_get(url: &str, referer: Option<&str>) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15")
        .build()?;
    let mut req = client
        .get(url)
        .header("Accept", "*/*")
        .header("Accept-Language", "zh-CN,zh;q=0.9");
    if let Some(r) = referer {
        req = req.header("Referer", r);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // 打印整条链便于排查：reqwest error 通常包含 is_connect / is_timeout / is_request
            anyhow::bail!(
                "reqwest send: kind={:?} cause={:?} url={}",
                e,
                e.source(),
                url
            );
        }
    };
    if !resp.status().is_success() {
        anyhow::bail!("http {} for {}", resp.status(), url);
    }
    let bytes = resp.bytes().await?;
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.to_string());
    }
    let (decoded, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(decoded.into_owned())
}
