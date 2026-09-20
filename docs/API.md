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

下单示例：

```json
{
  "code": "sz000001",
  "side": "buy",
  "qty": 100,
  "price": 11.8,
  "signal_id": null,
  "strategy_id": null
}
```

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
| GET | `/api/v1/ai/usage?limit=20` | 分析调用记录 |

请求体可以为空，也可以传 `{"signal_id":"..."}`。绑定历史信号时，路径中的代码必须与信号代码一致。

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
