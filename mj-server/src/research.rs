//! 投研上下文采集：新闻摘要、昨日美股和 A 股行业板块。
//!
//! 每条数据都保留来源与时间。采集失败会进入 `errors`，不会伪造缺失数据。

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::market;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub source: String,
    pub title: String,
    pub url: String,
    pub published_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketMove {
    pub symbol: String,
    pub name: String,
    pub session_date: String,
    pub close: f64,
    pub change_pct: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectorMove {
    pub code: String,
    pub name: String,
    pub change_pct: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchContext {
    pub captured_at: String,
    pub news: Vec<NewsItem>,
    pub news_sentiment: f64,
    pub us_market: Vec<MarketMove>,
    pub sectors: Vec<SectorMove>,
    pub errors: Vec<String>,
}

impl Default for ResearchContext {
    fn default() -> Self {
        Self {
            captured_at: Utc::now().to_rfc3339(),
            news: Vec::new(),
            news_sentiment: 0.0,
            us_market: Vec::new(),
            sectors: Vec::new(),
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Rss {
    channel: RssChannel,
}

#[derive(Debug, Deserialize)]
struct RssChannel {
    #[serde(default)]
    item: Vec<RssItem>,
}

#[derive(Debug, Deserialize)]
struct RssItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    link: String,
    #[serde(default, rename = "pubDate")]
    published_at: String,
}

pub async fn collect() -> ResearchContext {
    let (news, us_market, sectors) = tokio::join!(fetch_news(), fetch_us_market(), fetch_sectors());
    let mut context = ResearchContext::default();
    match news {
        Ok(items) => {
            context.news_sentiment = sentiment_score(&items);
            context.news = items;
        }
        Err(error) => context.errors.push(format!("news: {error}")),
    }
    match us_market {
        Ok(items) => context.us_market = items,
        Err(error) => context.errors.push(format!("us_market: {error}")),
    }
    match sectors {
        Ok(items) => context.sectors = items,
        Err(error) => context.errors.push(format!("sectors: {error}")),
    }
    context
}

fn configured_feeds() -> Vec<(String, String)> {
    if let Ok(raw) = std::env::var("MJ_NEWS_FEEDS") {
        let feeds = raw
            .split(';')
            .filter_map(|entry| {
                let (name, url) = entry.trim().split_once('|')?;
                if name.trim().is_empty() || url.trim().is_empty() {
                    None
                } else {
                    Some((name.trim().to_string(), url.trim().to_string()))
                }
            })
            .collect::<Vec<_>>();
        if !feeds.is_empty() {
            return feeds;
        }
    }

    Vec::new()
}

async fn fetch_news() -> anyhow::Result<Vec<NewsItem>> {
    let mut items = fetch_eastmoney_news().await.unwrap_or_default();
    let mut failures = Vec::new();
    for (source, url) in configured_feeds().into_iter().take(8) {
        match market::http_get(&url, None).await {
            Ok(body) => match parse_rss(&source, &body) {
                Ok(mut parsed) => items.append(&mut parsed),
                Err(error) => failures.push(format!("{source}: {error}")),
            },
            Err(error) => failures.push(format!("{source}: {error}")),
        }
    }
    items.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    items.dedup_by(|a, b| a.title == b.title);
    items.truncate(30);
    if items.is_empty() {
        anyhow::bail!("no headlines; {}", failures.join("; "));
    }
    Ok(items)
}

async fn fetch_eastmoney_news() -> anyhow::Result<Vec<NewsItem>> {
    let url = format!(
        "https://np-listapi.eastmoney.com/comm/web/getNewsByColumns?client=web&biz=web_news_col&column=350&order=1&needInteractData=0&page_index=1&page_size=30&req_trace={}",
        Utc::now().timestamp_millis()
    );
    let body = market::http_get(&url, Some("https://finance.eastmoney.com/")).await?;
    let value: serde_json::Value = serde_json::from_str(&body)?;
    let rows = value
        .pointer("/data/list")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("eastmoney news payload missing data.list"))?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(NewsItem {
                source: row
                    .get("mediaName")
                    .and_then(|value| value.as_str())
                    .unwrap_or("东方财富聚合")
                    .to_string(),
                title: row.get("title")?.as_str()?.to_string(),
                url: row
                    .get("uniqueUrl")
                    .or_else(|| row.get("url"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
                published_at: row
                    .get("showTime")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

fn parse_rss(source: &str, body: &str) -> anyhow::Result<Vec<NewsItem>> {
    let rss: Rss = quick_xml::de::from_str(body)?;
    Ok(rss
        .channel
        .item
        .into_iter()
        .filter(|item| !item.title.trim().is_empty())
        .take(15)
        .map(|item| NewsItem {
            source: source.to_string(),
            title: item.title.trim().to_string(),
            url: item.link.trim().to_string(),
            published_at: item.published_at.trim().to_string(),
        })
        .collect())
}

async fn fetch_us_market() -> anyhow::Result<Vec<MarketMove>> {
    let symbols = [
        ("usINX", "S&P 500"),
        ("usIXIC", "Nasdaq Composite"),
        ("usDJI", "Dow Jones"),
    ];
    let mut moves = Vec::new();
    for (symbol, name) in symbols {
        let quote = market::fetch_quote_tencent(symbol).await?;
        moves.push(MarketMove {
            symbol: symbol.to_string(),
            name: name.to_string(),
            session_date: quote
                .time_text
                .get(..10)
                .unwrap_or(&quote.time_text)
                .to_string(),
            close: quote.price,
            change_pct: quote.pct,
            source: "腾讯美股行情".into(),
        });
    }
    Ok(moves)
}

async fn fetch_sectors() -> anyhow::Result<Vec<SectorMove>> {
    let body = market::http_get(
        "https://vip.stock.finance.sina.com.cn/q/view/newSinaHy.php",
        Some("https://finance.sina.com.cn/"),
    )
    .await?;
    let start = body
        .find('{')
        .ok_or_else(|| anyhow::anyhow!("sina sector: no JSON"))?;
    let end = body
        .rfind('}')
        .ok_or_else(|| anyhow::anyhow!("sina sector: incomplete JSON"))?;
    let value: serde_json::Value = serde_json::from_str(&body[start..=end])?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("sina sector: expected object"))?;
    let mut sectors = object
        .values()
        .filter_map(|value| {
            let fields = value.as_str()?.split(',').collect::<Vec<_>>();
            Some(SectorMove {
                code: fields.first()?.to_string(),
                name: fields.get(1)?.to_string(),
                change_pct: fields.get(5)?.parse().ok()?,
                source: "新浪行业板块".into(),
            })
        })
        .collect::<Vec<_>>();
    sectors.sort_by(|left, right| right.change_pct.total_cmp(&left.change_pct));
    sectors.truncate(10);
    Ok(sectors)
}

fn sentiment_score(items: &[NewsItem]) -> f64 {
    const POSITIVE: &[&str] = &[
        "增长",
        "突破",
        "上调",
        "回购",
        "增持",
        "利好",
        "创新高",
        "盈利",
    ];
    const NEGATIVE: &[&str] = &[
        "下跌", "下调", "减持", "亏损", "风险", "调查", "暴跌", "违约",
    ];
    let mut score: f64 = 0.0;
    for item in items {
        for word in POSITIVE {
            if item.title.contains(word) {
                score += 1.0;
            }
        }
        for word in NEGATIVE {
            if item.title.contains(word) {
                score -= 1.0;
            }
        }
    }
    if items.is_empty() {
        0.0
    } else {
        // 用全部标题数归一化，避免少量命中把整体情绪推到 ±1。
        (score / items.len() as f64).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_and_scores_headlines() {
        let body = r#"<rss><channel><item><title>公司盈利增长并创新高</title><link>https://example.com/a</link><pubDate>Fri, 18 Sep 2026 01:00:00 GMT</pubDate></item></channel></rss>"#;
        let items = parse_rss("test", body).unwrap();
        assert_eq!(items.len(), 1);
        assert!(sentiment_score(&items) > 0.0);
    }
}
