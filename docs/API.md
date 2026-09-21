# API 参考

默认基址：`http://127.0.0.1:8787`。成功响应通常包含 `ok: true`，错误响应包含 `ok: false` 与 `error`。当前 API 尚未承诺稳定性。

## 系统

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/healthz` | 数据库、账户和信号健康摘要 |
| GET | `/api/v1/meta` | 服务公开端点列表 |
| GET | `/api/v1/ws/stats` | WebSocket 客户端和主题统计 |
| GET | `/ws` | WebSocket 升级入口 |

## 行情与信号

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/quote/{code}` | 实时行情及可用来源 |
| GET | `/api/v1/daily/{code}?limit=120` | 前复权日线 |
| POST | `/api/v1/signal/evaluate/{code}` | 即时评估并保存信号 |
| GET | `/api/v1/signals` | 最近信号 |
| POST | `/api/v1/scan` | 批量扫描标的 |
| GET | `/api/v1/signal/{id}` | 信号及后验详情 |
| POST | `/api/v1/signal/{id}/backfill` | 回填单个信号后验 |
| POST | `/api/v1/signals/backfill` | 批量回填后验 |
| GET | `/api/v1/performance?days=5&limit=200` | 后验聚合 |

股票代码使用带市场前缀的格式，例如 `sh600000`、`sz000001`、`bj899050`。

## 模拟盘

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/v1/paper/account` | 默认账户与持仓 |
| GET | `/api/v1/paper/positions` | 持仓列表 |
| GET/POST | `/api/v1/paper/orders` | 查询或提交订单 |
| DELETE | `/api/v1/paper/order/{id}` | 撤销 pending 订单 |
| GET | `/api/v1/paper/fills` | 成交列表 |
| GET | `/api/v1/paper/pnl` | 每日权益快照 |
| POST | `/api/v1/paper/snapshot` | 刷新全部持仓估值并记录快照 |
| GET | `/api/v1/paper/risk` | 用最新行情检查持仓止损、止盈与风险计划状态 |

下单示例：

```json
{
  "code": "sz000001",
  "side": "buy",
  "qty": 100,
  "price": 11.8,
  "signal_id": null,
  "strategy_id": null,
  "risk_plan": {
    "stop_price": 11.2,
    "take_profit_price": 13.0,
    "risk_budget_pct": 0.01,
    "suggested_position_pct": 0.12,
    "basis": "研究候选风险计划"
  }
}
```

`risk_plan` 仅适用于买单，且应满足 `stop_price < order price < take_profit_price`。成交后同一账户、同一股票的新计划会覆盖旧计划。风险接口会返回 `normal`、`near_stop`、`stop_triggered`、`target_reached` 或 `unplanned`；这些状态只用于提醒，不会自动下单。

## 自选与指数池

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET/POST | `/api/v1/watchlist` | 查询或增加自选 |
| DELETE | `/api/v1/watchlist/item` | 删除自选项 |
| GET | `/api/v1/watchlist/groups` | 自选分组 |
| GET/POST | `/api/v1/index-universe` | 查询或增加指数 |
| DELETE | `/api/v1/index-universe/{code}` | 删除指数 |

## AI 分析

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| POST | `/api/v1/ai/analyze/{code}` | 五角色分析和仲裁 |
| GET | `/api/v1/ai/config` | 查看真实模型启用状态，不返回 Token |
| GET | `/api/v1/ai/usage?limit=20` | 分析调用记录 |
| GET | `/api/v1/research/context` | 新闻、昨日美股和行业板块上下文 |
| POST | `/api/v1/research/select` | 对自选池或指定代码运行五路候选排序 |

请求体可以为空，也可以传 `{"signal_id":"..."}`。绑定历史信号时，路径中的代码必须与信号代码一致。

智能候选示例：

```json
{
  "codes": ["sz300623", "sh600519", "sz000001"],
  "max_candidates": 3
}
```

`codes` 留空时读取自选池。单次最多分析 12 只并返回前 10 只。排序依据为技术信号、历史后验、新闻、美股和行业上下文的五角色仲裁结果，不是个性化买卖指令。

每个 `candidate` 包含 `risk_plan`：20 日日波动率与年化波动率、14 日 ATR 百分比、止损/止盈参考、最大仓位、建议仓位和按 100 股取整的建议最大数量。仓位按模拟账户权益的 1% 风险预算计算，上限为权益的 20%；该结果仅用于纸上研究。

## WebSocket

客户端订阅：

```json
{"action":"subscribe","topics":["signal","fill","equity","healthz","ai"]}
```

服务端事件：

```json
{"topic":"ai","ts":"2026-09-20T08:00:00Z","payload":{}}
```

每个客户端维护独立订阅。未知主题会被忽略。
