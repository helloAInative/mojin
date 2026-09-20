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
    pub signal_id: Option<String>,
    pub roles: Vec<ModelOutput>,
    pub final_score: f64,
    pub final_confidence: f64,
    pub verdict: String, // buy / hold / sell / watch
    pub rationale: String,
    pub disclaimer: &'static str,
}

/// Model provider 抽象：每个 role 一个 provider。生产环境可换成真实 HTTP/SDK。
pub trait ModelProvider: Send + Sync {
    fn role(&self) -> &'static str;
    fn model_name(&self) -> &'static str;
    fn invoke<'a>(
        &'a self,
        ctx: &'a AiContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelOutput> + Send + 'a>>;
}

#[derive(Debug, Clone)]
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
        })
    })
}

// ─────────── 默认 5 个本地 Provider（heuristic） ───────────

pub struct MockTechnician;
impl ModelProvider for MockTechnician {
    fn role(&self) -> &'static str {
        "technician"
    }
    fn model_name(&self) -> &'static str {
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
    fn model_name(&self) -> &'static str {
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
                _ => 0.0,
            };
            let confidence = if ctx.ret_5d.is_some() || ctx.ret_20d.is_some() {
                0.4
            } else {
                0.1
            };
            let body = "基本面：当前无财务数据接入，仅依据后验收益倾向给出弱信号".to_string();
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
    fn model_name(&self) -> &'static str {
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
    fn model_name(&self) -> &'static str {
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
    fn model_name(&self) -> &'static str {
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
            let score = score.clamp(-1.0, 1.0);
            ModelOutput {
                role: self.role().into(),
                model: self.model_name().into(),
                score,
                confidence: 0.3,
                body: "情绪面：以量能推断散户/机构参与度，给出弱倾向".into(),
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
        signal_id: ctx.signal_id.clone(),
        roles,
        final_score,
        final_confidence: final_conf,
        verdict,
        rationale,
        disclaimer: "AI 投研为家庭自用辅助结论，不构成投资建议",
    })
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
        };
        let response = analyze(&state, ctx, default_providers()).await.unwrap();
        assert_eq!(response.signal_id.as_deref(), Some("sig-1"));
        assert_eq!(response.roles.len(), 5);
        assert_eq!(list_usage(&state, 10).unwrap().len(), 5);
    }
}
