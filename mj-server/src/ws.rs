//! 简单 WebSocket hub：单进程内存广播，给将来的 Flutter / Tauri 壳推送。
//!
//! 协议（文本 JSON）：
//! - 客户端 → 服务端：`{"action":"subscribe","topics":["signal","fill","equity","healthz","ai"]}`
//! - 服务端 → 客户端：`{"topic":"signal","ts":"...","payload":{...}}`
//!
//! 设计：
//! - `WsBroker` 持有一组 `(client_id, topics, sender)`；publish 按 topic fan-out
//! - `ws` 字段挂在 `AppState` 上，API handler 通过 `state.ws.publish(...)` 主动广播
//! - 心跳：服务端每 15s 发 ping；客户端 30s 不发任何消息就断

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{
    ws::{Message as AxWs, WebSocket, WebSocketUpgrade},
    State,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

use crate::db::AppState;

pub const DEFAULT_TOPICS: &[&str] = &["signal", "fill", "equity", "healthz", "ai"];

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "topic", rename_all = "lowercase")]
pub enum WsEvent {
    Signal {
        ts: String,
        payload: serde_json::Value,
    },
    Fill {
        ts: String,
        payload: serde_json::Value,
    },
    Equity {
        ts: String,
        payload: serde_json::Value,
    },
    Healthz {
        ts: String,
        payload: serde_json::Value,
    },
    Ai {
        ts: String,
        payload: serde_json::Value,
    },
}

impl WsEvent {
    pub fn topic(&self) -> &'static str {
        match self {
            WsEvent::Signal { .. } => "signal",
            WsEvent::Fill { .. } => "fill",
            WsEvent::Equity { .. } => "equity",
            WsEvent::Healthz { .. } => "healthz",
            WsEvent::Ai { .. } => "ai",
        }
    }
}

struct Client {
    topics: HashSet<String>,
    #[allow(dead_code)]
    last_seen: Instant,
}

impl Default for Client {
    fn default() -> Self {
        Self {
            topics: HashSet::new(),
            last_seen: Instant::now(),
        }
    }
}

#[derive(Clone)]
pub struct WsBroker {
    inner: Arc<RwLock<BrokerInner>>,
}

struct BrokerInner {
    clients: Vec<(Uuid, mpsc::UnboundedSender<String>, Client)>,
}

impl Default for BrokerInner {
    fn default() -> Self {
        Self {
            clients: Vec::new(),
        }
    }
}

impl WsBroker {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BrokerInner::default())),
        }
    }

    pub async fn publish(&self, ev: WsEvent) {
        let text = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
        let topic = ev.topic().to_string();
        let mut dead = Vec::new();
        {
            let g = self.inner.read().await;
            for (id, tx, c) in g.clients.iter() {
                if c.topics.contains(&topic) {
                    if tx.send(text.clone()).is_err() {
                        dead.push(*id);
                    }
                }
            }
        }
        if !dead.is_empty() {
            let mut g = self.inner.write().await;
            g.clients.retain(|(id, _, _)| !dead.contains(id));
        }
    }

    pub async fn stats(&self) -> (usize, Vec<String>) {
        let g = self.inner.read().await;
        let topics: std::collections::HashSet<String> = g
            .clients
            .iter()
            .flat_map(|(_, _, c)| c.topics.iter().cloned())
            .collect();
        let mut topics: Vec<String> = topics.into_iter().collect();
        topics.sort();
        (g.clients.len(), topics)
    }
}

#[derive(Deserialize)]
struct ClientMsg {
    action: String,
    #[serde(default)]
    topics: Vec<String>,
}

/// WS handler —— Axum 升级
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let client_id = Uuid::new_v4();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    // 默认全订阅
    let topics: HashSet<String> = DEFAULT_TOPICS.iter().map(|s| s.to_string()).collect();
    {
        let mut g = state.ws.inner.write().await;
        g.clients.push((
            client_id,
            tx,
            Client {
                topics,
                last_seen: Instant::now(),
            },
        ));
    }
    tracing::info!(%client_id, "ws connected");

    let (mut sender, mut receiver) = socket.split();
    let mut last_ping = Instant::now();
    loop {
        tokio::select! {
            // 1) 服务端 → 客户端
            msg = rx.recv() => {
                match msg {
                    Some(text) => {
                        if sender.send(AxWs::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            // 2) 客户端 → 服务端
            ws_in = receiver.next() => {
                match ws_in {
                    Some(Ok(AxWs::Text(t))) => {
                        if let Ok(parsed) = serde_json::from_str::<ClientMsg>(&t) {
                            if parsed.action == "subscribe" {
                                let mut g = state.ws.inner.write().await;
                                if let Some((_, _, client)) = g.clients.iter_mut().find(|(id, _, _)| *id == client_id) {
                                    client.topics = parsed.topics
                                        .into_iter()
                                        .filter(|topic| DEFAULT_TOPICS.contains(&topic.as_str()))
                                        .collect();
                                    client.last_seen = Instant::now();
                                }
                            }
                        }
                    }
                    Some(Ok(AxWs::Close(_))) | None => break,
                    _ => {}
                }
                let mut g = state.ws.inner.write().await;
                if let Some((_, _, client)) = g.clients.iter_mut().find(|(id, _, _)| *id == client_id) {
                    client.last_seen = Instant::now();
                }
            }
            // 3) 心跳
            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                if last_ping.elapsed() >= Duration::from_secs(15) {
                    if sender.send(AxWs::Ping(bytes::Bytes::new())).await.is_err() { break; }
                    last_ping = Instant::now();
                }
            }
        }
    }

    // 清理
    {
        let mut g = state.ws.inner.write().await;
        g.clients.retain(|(id, _, _)| *id != client_id);
    }
    tracing::info!(%client_id, "ws disconnected");
}

/// 定时广播 equity + healthz 简版
pub async fn periodic_broadcast(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.tick().await; // 跳过立即第一次
    loop {
        interval.tick().await;
        let (clients, _topics) = state.ws.stats().await;
        if clients == 0 {
            continue;
        }
        // equity
        let eq: f64 = state
            .with_conn(|c| -> Result<f64, crate::error::AppError> {
                c.query_row(
                    "SELECT equity FROM paper_account WHERE id='default'",
                    [],
                    |r| r.get::<_, f64>(0),
                )
                .map_err(crate::error::AppError::Db)
            })
            .unwrap_or(0.0);
        let _ = state
            .ws
            .publish(WsEvent::Equity {
                ts: chrono::Utc::now().to_rfc3339(),
                payload: serde_json::json!({ "equity": eq, "ts": chrono::Utc::now().to_rfc3339() }),
            })
            .await;
        // healthz
        let labeled: i64 = state
            .with_conn(|c| -> Result<i64, crate::error::AppError> {
                c.query_row(
                    "SELECT COUNT(*) FROM signal_performance WHERE labeled_at IS NOT NULL",
                    [],
                    |r| r.get(0),
                )
                .map_err(crate::error::AppError::Db)
            })
            .unwrap_or(0);
        let _ = state
            .ws
            .publish(WsEvent::Healthz {
                ts: chrono::Utc::now().to_rfc3339(),
                payload: serde_json::json!({ "signals_labeled": labeled, "ts": chrono::Utc::now().to_rfc3339() }),
            })
            .await;
    }
}
