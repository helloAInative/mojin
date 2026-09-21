//! AI 五模型并行 + 仲裁 + ai_usage 落库。
//!
//! 设计目标：
//! - 5 个角色 (`technician` / `fundamental` / `risk` / `position` / `psychology`)
//!   通过 trait `ModelProvider` 异步产出 (`ModelOutput { score, confidence, body, ... }`)
//! - 默认实现 `MockProvider`：本地 deterministic 启发式，token=0、cost=0；
//!   以后可插入 `OpenAIModel` / `OllamaModel` 等真实调用，自动按 token 计费。
//! - `fan_out()` 用 `tokio::join!` 并发跑所有角色，容忍单点失败。
//! - `Arbitrator` 综合 5 路输出做 final call，权重来自 `model_route_stat`。
//! - 每次调用写一行 `ai_usage`：model / tokens_in / tokens_out / latency_ms / ok / cost。

use std::time::Instant;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::AppState;
use crate::error::AppError;
use crate::research::ResearchContext;

/// 五角色固定清单。
pub const ROLES: &[&str] = &[
    "technician",  // 技术
    "fundamental", // 基本面
    "risk",        // 风险
    "position",    // 持仓/资金
    "psychology",  // 情绪/盘口
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelOutput {
    pub role: String,
    pub model: String,
    pub score: f64,      // [-1, 1] 多空倾向
    pub confidence: f64, // [0, 1] 自信度
    pub body: String,    // 文字解释
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub latency_ms: i64,
    pub ok: bool,
    pub fallback: String, // 出错时的兜底来源
}

#[derive(Debug, Clone, Serialize)]
pub struct ArbitrateResp {
    pub ok: bool,
    pub code: String,
    pub name: String,
    pub price: f64,
    pub signal_id: Option<String>,
    pub roles: Vec<ModelOutput>,
    pub final_score: f64,
    pub final_confidence: f64,
    pub verdict: String, // buy / hold / sell / watch
    pub risk_plan: RiskPlan,
    pub rationale: String,
    pub disclaimer: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskPlan {
    pub daily_volatility_20d: Option<f64>,
    pub annualized_volatility: Option<f64>,
    pub atr_pct_14d: Option<f64>,
    pub stop_loss_pct: f64,
    pub stop_price: f64,
    pub take_profit_pct: f64,
    pub take_profit_price: f64,
    pub risk_budget_pct: f64,
    pub max_position_pct: f64,
    pub suggested_position_pct: f64,
    pub suggested_max_shares: i64,
    pub basis: &'static str,
}

/// Model provider 抽象：每个 role 一个 provider。生产环境可换成真实 HTTP/SDK。
pub trait ModelProvider: Send + Sync {
    fn role(&self) -> &'static str;
    fn model_name(&self) -> &str;
    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>>;
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct HistoricalEvidence {
    pub samples_5d: i64,
    pub hit_rate_5d: Option<f64>,
    pub avg_return_5d: Option<f64>,
    pub samples_20d: i64,
    pub hit_rate_20d: Option<f64>,
    pub avg_return_20d: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiContext {
    pub signal_id: Option<String>,
    pub code: String,
    pub name: String,
    pub price: f64,
    pub signal_level: String,
    pub signal_confidence: f64,
    pub factors: Vec<(String, f64, String)>, // (key, value, detail)
    pub ret_5d: Option<f64>,
    pub ret_20d: Option<f64>,
    pub daily_volatility_20d: Option<f64>,
    pub atr_pct_14d: Option<f64>,
    pub historical_evidence: HistoricalEvidence,
    pub research: ResearchContext,
}

fn historical_evidence_from_conn(
    conn: &rusqlite::Connection,
    code: &str,
) -> Result<HistoricalEvidence, AppError> {
    conn.query_row(
        "SELECT COUNT(p.ret_5d),
                AVG(CASE WHEN p.ret_5d > 0 THEN 1.0 ELSE 0.0 END),
                AVG(p.ret_5d),
                COUNT(p.ret_20d),
                AVG(CASE WHEN p.ret_20d > 0 THEN 1.0 ELSE 0.0 END),
                AVG(p.ret_20d)
           FROM signal s
           LEFT JOIN signal_performance p ON p.signal_id = s.id
          WHERE s.code = ?",
        params![code],
        |row| {
            Ok(HistoricalEvidence {
                samples_5d: row.get(0)?,
                hit_rate_5d: row.get(1)?,
                avg_return_5d: row.get(2)?,
                samples_20d: row.get(3)?,
                hit_rate_20d: row.get(4)?,
                avg_return_20d: row.get(5)?,
            })
        },
    )
    .map_err(AppError::Db)
}

pub fn historical_evidence(state: &AppState, code: &str) -> Result<HistoricalEvidence, AppError> {
    state.with_conn(|conn| historical_evidence_from_conn(conn, code))
}

/// Build analysis input from one persisted signal. This keeps historical analysis
/// tied to the factors and price that existed when the signal fired.
pub fn context_for_signal(
    state: &AppState,
    signal_id: &str,
    expected_code: &str,
) -> Result<AiContext, AppError> {
    state.with_conn(|c| {
        let row = c
            .query_row(
                "SELECT s.code, s.name, s.price, s.level, s.confidence,
                        p.ret_5d, p.ret_20d
                   FROM signal s
                   LEFT JOIN signal_performance p ON p.signal_id = s.id
                  WHERE s.id = ?",
                params![signal_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, f64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, f64>(4)?,
                        r.get::<_, Option<f64>>(5)?,
                        r.get::<_, Option<f64>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| AppError::Msg(format!("signal not found: {signal_id}")))?;
        if row.0 != expected_code {
            return Err(AppError::Msg(format!(
                "signal {signal_id} belongs to {}, not {expected_code}",
                row.0
            )));
        }
        let mut stmt = c.prepare(
            "SELECT factor_key, COALESCE(factor_value, 0), detail
               FROM signal_factor_hit WHERE signal_id = ? ORDER BY id",
        )?;
        let factors = stmt
            .query_map(params![signal_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let historical_evidence = historical_evidence_from_conn(c, &row.0)?;
        Ok(AiContext {
            signal_id: Some(signal_id.to_string()),
            code: row.0,
            name: row.1,
            price: row.2,
            signal_level: row.3,
            signal_confidence: row.4,
            factors,
            ret_5d: row.5,
            ret_20d: row.6,
            daily_volatility_20d: None,
            atr_pct_14d: None,
            historical_evidence,
            research: ResearchContext::default(),
        })
    })
}

// ─────────── 默认 5 个本地 Provider（heuristic） ───────────

pub struct MockTechnician;
impl ModelProvider for MockTechnician {
    fn role(&self) -> &'static str {
        "technician"
    }
    fn model_name(&self) -> &str {
        "mock-technician-v1"
    }
    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            // 综合 MACD 金叉 + 量能 → score
            let mut score: f64 = 0.0;
            let mut notes = Vec::new();
            for (k, _v, det) in &ctx.factors {
                match k.as_str() {
                    "macd_golden" => {
                        score += 0.5;
                        notes.push("MACD 金叉");
                    }
                    "macd_death" => {
                        score -= 0.5;
                        notes.push("MACD 死叉");
                    }
                    "vol_spike" => {
                        score += 0.3;
                        notes.push("放量");
                    }
                    "vol_shrink" => {
                        score -= 0.1;
                        notes.push("缩量");
                    }
                    "kdj_overbought" => {
                        score -= 0.3;
                        notes.push("KDJ 超买");
                    }
                    "kdj_oversold" => {
                        score += 0.3;
                        notes.push("KDJ 超卖");
                    }
                    "rsi_high" => {
                        score -= 0.2;
                        notes.push("RSI 高位");
                    }
                    "rsi_low" => {
                        score += 0.2;
                        notes.push("RSI 低位");
                    }
                    _ => {
                        let _ = det.as_str();
                    }
                }
            }
            let score = score.clamp(-1.0, 1.0);
            let confidence = (ctx.factors.len() as f64 / 5.0).min(1.0);
            let body = if notes.is_empty() {
                format!("技术面：现价 {:.2}，无明确技术因子", ctx.price)
            } else {
                format!(
                    "技术面：现价 {:.2}，{}；综合得分 {:.2}",
                    ctx.price,
                    notes.join("、"),
                    score
                )
            };
            ModelOutput {
                role: self.role().into(),
                model: self.model_name().into(),
                score,
                confidence,
                body,
                tokens_in: 0,
                tokens_out: 0,
                latency_ms: started.elapsed().as_millis() as i64,
                ok: true,
                fallback: "".into(),
            }
        })
    }
}

pub struct MockFundamental;
impl ModelProvider for MockFundamental {
    fn role(&self) -> &'static str {
        "fundamental"
    }
    fn model_name(&self) -> &str {
        "mock-fundamental-v1"
    }
    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            // 没有基本面表 → 偏中性、置信低；后验 ret_5d/20d 作 hint
            let score = match (ctx.ret_5d, ctx.ret_20d) {
                (Some(r5), Some(r20)) => (r5 * 0.4 + r20 * 0.6).clamp(-1.0, 1.0),
                (Some(r5), None) => r5.clamp(-1.0, 1.0),
                _ => ctx
                    .historical_evidence
                    .avg_return_5d
                    .unwrap_or(0.0)
                    .clamp(-1.0, 1.0),
            };
            let confidence = ((ctx.historical_evidence.samples_5d as f64) / 30.0).clamp(0.1, 0.55);
            let body = format!(
                "历史后验：5 日样本 {}，胜率 {}，平均收益 {}；基本面数据仍待接入",
                ctx.historical_evidence.samples_5d,
                ctx.historical_evidence
                    .hit_rate_5d
                    .map(|value| format!("{:.1}%", value * 100.0))
                    .unwrap_or_else(|| "—".into()),
                ctx.historical_evidence
                    .avg_return_5d
                    .map(|value| format!("{:.2}%", value * 100.0))
                    .unwrap_or_else(|| "—".into())
            );
            ModelOutput {
                role: self.role().into(),
                model: self.model_name().into(),
                score,
                confidence,
                body,
                tokens_in: 0,
                tokens_out: 0,
                latency_ms: started.elapsed().as_millis() as i64,
                ok: true,
                fallback: "".into(),
            }
        })
    }
}

pub struct MockRisk;
impl ModelProvider for MockRisk {
    fn role(&self) -> &'static str {
        "risk"
    }
    fn model_name(&self) -> &str {
        "mock-risk-v1"
    }
    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            // 风险侧：level 越高、置信越高，越倾向 hold；level=Risk 直接 sell 倾向
            let (score, verdict_hint) = match ctx.signal_level.as_str() {
                "risk" => (-0.6, "提示减仓/观察"),
                "tip" => (0.0, "中性观察"),
                "confirm" => (0.1, "控制仓位"),
                "strong" => (0.2, "允许小仓"),
                _ => (0.0, "中性"),
            };
            ModelOutput {
                role: self.role().into(),
                model: self.model_name().into(),
                score,
                confidence: (0.4 + ctx.signal_confidence * 0.4).clamp(0.4, 0.8),
                body: format!(
                    "风险侧：当前信号等级 {}，{}",
                    ctx.signal_level, verdict_hint
                ),
                tokens_in: 0,
                tokens_out: 0,
                latency_ms: started.elapsed().as_millis() as i64,
                ok: true,
                fallback: "".into(),
            }
        })
    }
}

pub struct MockPosition;
impl ModelProvider for MockPosition {
    fn role(&self) -> &'static str {
        "position"
    }
    fn model_name(&self) -> &str {
        "mock-position-v1"
    }
    fn invoke<'a>(
        &'a self,
        _ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            ModelOutput {
                role: self.role().into(),
                model: self.model_name().into(),
                score: 0.0,
                confidence: 0.5,
                body: "持仓侧：默认账户 10 万，目前无 KPI 触发，输出中性".into(),
                tokens_in: 0,
                tokens_out: 0,
                latency_ms: started.elapsed().as_millis() as i64,
                ok: true,
                fallback: "".into(),
            }
        })
    }
}

pub struct MockPsychology;
impl ModelProvider for MockPsychology {
    fn role(&self) -> &'static str {
        "psychology"
    }
    fn model_name(&self) -> &str {
        "mock-psychology-v1"
    }
    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>> {
        Box::pin(async move {
            let started = Instant::now();
            // 情绪面：放量 → 偏多；缩量 → 偏空
            let mut score = 0.0;
            for (k, v, _) in &ctx.factors {
                if k == "vol_spike" {
                    score += (v.min(3.0) - 1.0) * 0.3;
                }
                if k == "vol_shrink" {
                    score -= 0.2;
                }
            }
            score += ctx.research.news_sentiment * 0.25;
            if !ctx.research.us_market.is_empty() {
                let average = ctx
                    .research
                    .us_market
                    .iter()
                    .map(|item| item.change_pct)
                    .sum::<f64>()
                    / ctx.research.us_market.len() as f64;
                score += (average / 3.0).clamp(-0.3, 0.3);
            }
            let score = score.clamp(-1.0, 1.0);
            ModelOutput {
                role: self.role().into(),
                model: self.model_name().into(),
                score,
                confidence: if ctx.research.news.is_empty() {
                    0.3
                } else {
                    0.55
                },
                body: format!(
                    "情绪面：新闻 {} 条，标题情绪 {:.2}，美股指数 {} 个；结合量能给出弱倾向",
                    ctx.research.news.len(),
                    ctx.research.news_sentiment,
                    ctx.research.us_market.len()
                ),
                tokens_in: 0,
                tokens_out: 0,
                latency_ms: started.elapsed().as_millis() as i64,
                ok: true,
                fallback: "".into(),
            }
        })
    }
}

/// 默认 provider 集合。
pub fn default_providers() -> Vec<Box<dyn ModelProvider>> {
    vec![
        Box::new(MockTechnician),
        Box::new(MockFundamental),
        Box::new(MockRisk),
        Box::new(MockPosition),
        Box::new(MockPsychology),
    ]
}

fn mock_provider(role: &str) -> Box<dyn ModelProvider> {
    match role {
        "technician" => Box::new(MockTechnician),
        "fundamental" => Box::new(MockFundamental),
        "risk" => Box::new(MockRisk),
        "position" => Box::new(MockPosition),
        _ => Box::new(MockPsychology),
    }
}

#[derive(Debug, Clone)]
struct RemoteConfig {
    api_key: String,
    base_url: String,
    default_model: String,
    timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AiConfigView {
    pub remote_enabled: bool,
    pub base_url: Option<String>,
    pub default_model: String,
    pub configured_base_url: String,
    pub configured_model: String,
    pub timeout_secs: u64,
    pub role_models: Vec<(String, String)>,
    pub token_configured: bool,
    pub env_file_loaded: bool,
    pub env_file_path: String,
    pub env_file_error: Option<String>,
}

fn remote_config() -> Option<RemoteConfig> {
    let api_key = std::env::var("MJ_AI_API_KEY").ok()?.trim().to_string();
    if api_key.is_empty() {
        return None;
    }
    let base_url = std::env::var("MJ_AI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".into())
        .trim_end_matches('/')
        .to_string();
    let default_model = std::env::var("MJ_AI_MODEL").unwrap_or_else(|_| "gpt-5-mini".into());
    let timeout_secs = std::env::var("MJ_AI_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(45)
        .clamp(5, 180);
    Some(RemoteConfig {
        api_key,
        base_url,
        default_model,
        timeout_secs,
    })
}

fn role_model(role: &str, default_model: &str) -> String {
    let key = format!("MJ_AI_MODEL_{}", role.to_ascii_uppercase());
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_model.to_string())
}

pub fn config_view() -> AiConfigView {
    let config = remote_config();
    let env_status = crate::config::env_status();
    let configured_base_url = std::env::var("MJ_AI_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".into())
        .trim_end_matches('/')
        .to_string();
    let configured_model = std::env::var("MJ_AI_MODEL").unwrap_or_else(|_| "gpt-5-mini".into());
    let default_model = config
        .as_ref()
        .map(|value| value.default_model.clone())
        .unwrap_or_else(|| "local-heuristic".into());
    AiConfigView {
        remote_enabled: config.is_some(),
        base_url: config.as_ref().map(|value| value.base_url.clone()),
        timeout_secs: config
            .as_ref()
            .map(|value| value.timeout_secs)
            .unwrap_or(45),
        role_models: ROLES
            .iter()
            .map(|role| {
                (
                    (*role).to_string(),
                    if config.is_some() {
                        role_model(role, &default_model)
                    } else {
                        format!("mock-{role}-v1")
                    },
                )
            })
            .collect(),
        default_model,
        configured_base_url,
        configured_model,
        token_configured: config.is_some(),
        env_file_loaded: env_status.env_file_loaded,
        env_file_path: env_status.env_file_path.clone(),
        env_file_error: env_status.env_file_error.clone(),
    }
}

struct OpenAiCompatibleProvider {
    role: &'static str,
    model: String,
    config: RemoteConfig,
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn role(&self) -> &'static str {
        self.role
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>> {
        Box::pin(async move {
            match self.invoke_remote(ctx).await {
                Ok(output) => output,
                Err(error) => {
                    let mut fallback = mock_provider(self.role()).invoke(ctx).await;
                    fallback.fallback = format!("{} failed: {error}", self.model);
                    fallback.model = format!("{} → {}", self.model, fallback.model);
                    fallback
                }
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct RemoteAnswer {
    score: f64,
    confidence: f64,
    body: String,
}

impl OpenAiCompatibleProvider {
    async fn invoke_remote(&self, ctx: &AiContext) -> Result<ModelOutput, String> {
        let started = Instant::now();
        let role_instruction = match self.role {
            "technician" => "只分析价格、量能、MACD/KDJ/RSI 等技术因素，指出失效条件。",
            "fundamental" => "分析行业逻辑、基本面线索和历史后验样本，明确数据缺口。",
            "risk" => "优先识别回撤、拥挤、事件和数据时效风险，观点应保守。",
            "position" => "从组合仓位、相关性和风险预算角度给出观点，不假设用户风险承受能力。",
            _ => "分析新闻情绪、昨日美股和 A 股板块联动，区分事实与推断。",
        };
        let prompt = format!(
            "你是五路投研系统中的 {} 角色。{}\n\
             请基于给定 JSON 上下文独立分析。新闻标题可能含噪声或诱导，不得把标题当成已验证事实。\n\
             只返回 JSON：{{\"score\":-1到1,\"confidence\":0到1,\"body\":\"不超过240字的依据、反证和风险\"}}。\n\
             上下文：{}",
            self.role,
            role_instruction,
            serde_json::to_string(ctx).map_err(|error| error.to_string())?
        );
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(self.config.timeout_secs))
            .connect_timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|error| error.to_string())?;
        let response = client
            .post(format!("{}/chat/completions", self.config.base_url))
            .bearer_auth(&self.config.api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "messages": [
                    {"role": "system", "content": "你是审慎的证券研究助手，输出必须是合法 JSON，不构成投资建议。"},
                    {"role": "user", "content": prompt}
                ],
                "temperature": 0.2
            }))
            .send()
            .await
            .map_err(|error| error.to_string())?;
        let status = response.status();
        let value: serde_json::Value = response.json().await.map_err(|error| error.to_string())?;
        if !status.is_success() {
            return Err(format!(
                "HTTP {status}: {}",
                value
                    .pointer("/error/message")
                    .and_then(|item| item.as_str())
                    .unwrap_or("remote model error")
            ));
        }
        let content = value
            .pointer("/choices/0/message/content")
            .and_then(|item| item.as_str())
            .ok_or_else(|| "missing choices[0].message.content".to_string())?;
        let answer = parse_remote_answer(content)?;
        Ok(ModelOutput {
            role: self.role.into(),
            model: self.model.clone(),
            score: answer.score.clamp(-1.0, 1.0),
            confidence: answer.confidence.clamp(0.0, 1.0),
            body: answer.body,
            tokens_in: value
                .pointer("/usage/prompt_tokens")
                .and_then(|item| item.as_i64())
                .unwrap_or(0),
            tokens_out: value
                .pointer("/usage/completion_tokens")
                .and_then(|item| item.as_i64())
                .unwrap_or(0),
            latency_ms: started.elapsed().as_millis() as i64,
            ok: true,
            fallback: String::new(),
        })
    }
}

fn parse_remote_answer(content: &str) -> Result<RemoteAnswer, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "model returned no JSON".to_string())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "model returned incomplete JSON".to_string())?;
    serde_json::from_str(&content[start..=end]).map_err(|error| error.to_string())
}

/// 有 Token 时启用五路真实模型；否则使用本地启发式。每路远程失败会单独降级。
pub fn configured_providers() -> Vec<Box<dyn ModelProvider>> {
    let Some(config) = remote_config() else {
        return default_providers();
    };
    ROLES
        .iter()
        .map(|role| {
            Box::new(OpenAiCompatibleProvider {
                role,
                model: role_model(role, &config.default_model),
                config: config.clone(),
            }) as Box<dyn ModelProvider>
        })
        .collect()
}

// ─────────── fan-out + 仲裁 + ai_usage 落库 ───────────

/// 并发调用所有 provider；任一失败返回的 ModelOutput ok=false 但仍记录。
pub async fn fan_out(ctx: &AiContext, providers: &[Box<dyn ModelProvider>]) -> Vec<ModelOutput> {
    let mut futs = Vec::with_capacity(providers.len());
    for p in providers {
        futs.push(p.invoke(ctx));
    }
    let outputs = futures::future::join_all(futs).await;
    outputs
}

/// 仲裁：把每路 score × weight 求和，归一化到 [-1, 1]。
pub fn arbitrate(roles: &[ModelOutput], weights: &[f64]) -> (f64, f64, String) {
    let mut sum_score = 0.0;
    let mut sum_w = 0.0;
    let mut sum_conf = 0.0;
    for (o, &w) in roles.iter().zip(weights.iter()) {
        if !o.ok || !w.is_finite() || w <= 0.0 || !o.score.is_finite() || !o.confidence.is_finite()
        {
            continue;
        }
        sum_score += o.score * w;
        sum_w += w;
        sum_conf += o.confidence * w;
    }
    let final_score = if sum_w > 0.0 { sum_score / sum_w } else { 0.0 };
    let final_conf = if sum_w > 0.0 { sum_conf / sum_w } else { 0.0 };
    let verdict = if final_score >= 0.3 {
        "buy"
    } else if final_score <= -0.3 {
        "sell"
    } else if final_score.abs() <= 0.1 {
        "hold"
    } else {
        "watch"
    }
    .to_string();
    (
        final_score.clamp(-1.0, 1.0),
        final_conf.clamp(0.0, 1.0),
        verdict,
    )
}

/// 从 `model_route_stat` 取权重，缺则给均匀权重。
pub fn load_weights(state: &AppState) -> Vec<f64> {
    state.with_conn(|c| {
        let mut out = Vec::with_capacity(ROLES.len());
        for role in ROLES {
            let w: f64 = c
                .query_row(
                    "SELECT weight FROM model_route_stat WHERE signal_type = 'default' AND role = ?",
                    params![role],
                    |r| r.get(0),
                )
                .unwrap_or(1.0);
            out.push(w);
        }
        Ok(out)
    })
    .unwrap_or_else(|_| vec![1.0; ROLES.len()])
}

/// 把单次调用结果写一行 ai_usage。
pub fn record_usage(state: &AppState, o: &ModelOutput, task: &str, prompt_kind: &str) {
    let _ = state.with_conn(|c| {
        c.execute(
            "INSERT INTO ai_usage(model, tokens_in, tokens_out, latency_ms, ok, fallback, task, prompt_kind, cost_estimate)
             VALUES(?,?,?,?,?,?,?,?,?)",
            params![
                o.model,
                o.tokens_in,
                o.tokens_out,
                o.latency_ms,
                o.ok as i64,
                o.fallback,
                task,
                prompt_kind,
                (o.tokens_in + o.tokens_out) as f64 * 0.0000, // 默认 mock = 0
            ],
        )?;
        Ok(())
    });
}

/// 一站式：ctx → fan-out → 仲裁 → 落库 → 返回 ArbitrateResp。
pub async fn analyze(
    state: &AppState,
    ctx: AiContext,
    providers: Vec<Box<dyn ModelProvider>>,
) -> Result<ArbitrateResp, AppError> {
    let weights = load_weights(state);
    let roles = fan_out(&ctx, &providers).await;
    // 写 ai_usage（每路一行）
    for o in &roles {
        record_usage(state, o, "analyze", ctx.signal_level.as_str());
    }
    let (final_score, final_conf, verdict) = arbitrate(&roles, &weights);
    let risk_plan = build_risk_plan(state, &ctx, &verdict, final_conf);
    let rationale = roles
        .iter()
        .map(|o| {
            format!(
                "[{}] score={:.2} conf={:.2} — {}",
                o.role, o.score, o.confidence, o.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(ArbitrateResp {
        ok: true,
        code: ctx.code.clone(),
        name: ctx.name.clone(),
        price: ctx.price,
        signal_id: ctx.signal_id.clone(),
        roles,
        final_score,
        final_confidence: final_conf,
        verdict,
        risk_plan,
        rationale,
        disclaimer: "AI 投研为家庭自用辅助结论，不构成投资建议",
    })
}

fn build_risk_plan(state: &AppState, ctx: &AiContext, verdict: &str, confidence: f64) -> RiskPlan {
    let (cash, equity) = state
        .with_conn(|conn| {
            conn.query_row(
                "SELECT cash, equity FROM paper_account WHERE id = 'default'",
                [],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?)),
            )
            .map_err(AppError::Db)
        })
        .unwrap_or((0.0, 0.0));
    let atr_stop = ctx.atr_pct_14d.unwrap_or(0.04) * 2.0;
    let vol_stop = ctx.daily_volatility_20d.unwrap_or(0.02) * 2.5;
    let stop_loss_pct = atr_stop.max(vol_stop).clamp(0.04, 0.12);
    let take_profit_pct = (stop_loss_pct * 2.0).min(0.24);
    let risk_budget_pct = 0.01;
    let max_position_pct = (risk_budget_pct / stop_loss_pct).min(0.20);
    let confidence_scale = 0.5 + confidence.clamp(0.0, 1.0) * 0.5;
    let suggested_position_pct = match verdict {
        "buy" => max_position_pct * confidence_scale,
        "watch" => max_position_pct.min(0.05) * confidence_scale,
        _ => 0.0,
    };
    let investable = (equity * suggested_position_pct).min(cash.max(0.0));
    let suggested_max_shares = if ctx.price > 0.0 {
        ((investable / ctx.price / 100.0).floor() * 100.0) as i64
    } else {
        0
    };
    RiskPlan {
        daily_volatility_20d: ctx.daily_volatility_20d,
        annualized_volatility: ctx
            .daily_volatility_20d
            .map(|value| value * 252.0_f64.sqrt()),
        atr_pct_14d: ctx.atr_pct_14d,
        stop_loss_pct,
        stop_price: ctx.price * (1.0 - stop_loss_pct),
        take_profit_pct,
        take_profit_price: ctx.price * (1.0 + take_profit_pct),
        risk_budget_pct,
        max_position_pct,
        suggested_position_pct,
        suggested_max_shares,
        basis: "按模拟账户权益的 1% 风险预算、20 日波动率与 14 日 ATR 估算；价格仅为研究参考",
    }
}

/// 读最近 N 条 ai_usage。
#[derive(Debug, Clone, Serialize)]
pub struct UsageRow {
    pub id: i64,
    pub model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub latency_ms: i64,
    pub ok: bool,
    pub fallback: String,
    pub task: String,
    pub prompt_kind: String,
    pub cost_estimate: f64,
    pub created_at: String,
}

pub fn list_usage(state: &AppState, limit: i64) -> Result<Vec<UsageRow>, AppError> {
    state.with_conn(|c| {
        let mut stmt = c.prepare(
            "SELECT id, model, tokens_in, tokens_out, latency_ms, ok, fallback, task, prompt_kind, cost_estimate, created_at
               FROM ai_usage ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(UsageRow {
                    id: r.get(0)?,
                    model: r.get(1)?,
                    tokens_in: r.get(2)?,
                    tokens_out: r.get(3)?,
                    latency_ms: r.get(4)?,
                    ok: r.get::<_, i64>(5)? != 0,
                    fallback: r.get(6)?,
                    task: r.get(7)?,
                    prompt_kind: r.get(8)?,
                    cost_estimate: r.get(9)?,
                    created_at: r.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
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

    fn output(score: f64, confidence: f64, ok: bool) -> ModelOutput {
        ModelOutput {
            role: "test".into(),
            model: "test".into(),
            score,
            confidence,
            body: String::new(),
            tokens_in: 0,
            tokens_out: 0,
            latency_ms: 0,
            ok,
            fallback: String::new(),
        }
    }

    #[test]
    fn persisted_signal_context_is_loaded_and_code_checked() {
        let state = state();
        state.with_conn(|c| {
            c.execute(
                "INSERT INTO signal(id, code, name, level, confidence, title, price, fired_at)
                 VALUES('sig-1','sz000001','平安银行','confirm',0.72,'test',11.8,'2026-09-18T01:00:00Z')",
                [],
            )?;
            c.execute(
                "INSERT INTO signal_factor_hit(signal_id, factor_key, factor_value, detail)
                 VALUES('sig-1','macd_golden',1.0,'金叉')",
                [],
            )?;
            c.execute(
                "INSERT INTO signal_performance(signal_id, ret_5d, ret_20d) VALUES('sig-1',0.05,0.12)",
                [],
            )?;
            Ok(())
        }).unwrap();
        let ctx = context_for_signal(&state, "sig-1", "sz000001").unwrap();
        assert_eq!(ctx.signal_id.as_deref(), Some("sig-1"));
        assert_eq!(ctx.factors[0].0, "macd_golden");
        assert_eq!(ctx.ret_20d, Some(0.12));
        assert!(context_for_signal(&state, "sig-1", "sh600000").is_err());
        assert!(context_for_signal(&state, "missing", "sz000001").is_err());
    }

    #[test]
    fn arbitration_ignores_failed_outputs_and_invalid_weights() {
        let roles = vec![
            output(0.8, 0.9, true),
            output(-1.0, 1.0, false),
            output(1.0, 1.0, true),
        ];
        let (score, confidence, verdict) = arbitrate(&roles, &[1.0, 100.0, f64::NAN]);
        assert!((score - 0.8).abs() < 1e-9);
        assert!((confidence - 0.9).abs() < 1e-9);
        assert_eq!(verdict, "buy");
    }

    #[tokio::test]
    async fn analysis_keeps_signal_id_and_records_each_role() {
        let state = state();
        let ctx = AiContext {
            signal_id: Some("sig-1".into()),
            code: "sz000001".into(),
            name: "平安银行".into(),
            price: 11.8,
            signal_level: "confirm".into(),
            signal_confidence: 0.7,
            factors: vec![("macd_golden".into(), 1.0, "金叉".into())],
            ret_5d: None,
            ret_20d: None,
            daily_volatility_20d: Some(0.02),
            atr_pct_14d: Some(0.03),
            historical_evidence: HistoricalEvidence::default(),
            research: ResearchContext::default(),
        };
        let response = analyze(&state, ctx, default_providers()).await.unwrap();
        assert_eq!(response.signal_id.as_deref(), Some("sig-1"));
        assert_eq!(response.roles.len(), 5);
        assert!(response.risk_plan.stop_loss_pct >= 0.06);
        assert!(response.risk_plan.max_position_pct <= 0.20);
        assert_eq!(list_usage(&state, 10).unwrap().len(), 5);
    }

    #[test]
    fn parses_json_from_remote_model_fences() {
        let answer = parse_remote_answer(
            "```json\n{\"score\":0.4,\"confidence\":0.7,\"body\":\"样本有限\"}\n```",
        )
        .unwrap();
        assert_eq!(answer.score, 0.4);
        assert_eq!(answer.confidence, 0.7);
    }
}
