# tauri_shell

Tauri 端占位壳 — 静态 Web 视图，对接后端 HTTP + WS。

## 文件

- `index.html` — 复盘工作台（总览/账户/信号/后验/AI/WS 日志，5+1 视图）
- `tauri.conf.json` — Tauri 配置（如已生成）

## 视图

1. **总览**：Healthz + 账户 + 后验（5 日）
2. **账户/持仓**：现金/冻结/权益/市值 + 持仓行情标记 + 一键刷新估值和记录快照
3. **信号**：最近 50 条信号
4. **后验**：1d/3d/5d/10d/20d 多窗口胜率/最好/最差
5. **AI**：WS 客户端统计 + ai_usage 表 + 一键触发 `/api/v1/ai/analyze/{code}`；可传 `signal_id` 按历史信号当时的价格、等级和因子分析
6. **WS 日志**：最近 100 条事件

## 后端约定

| 项 | 值 |
| --- | --- |
| 端口 | `8787` |
| Healthz | `GET /healthz` |
| 账户 | `GET /api/v1/paper/account` |
| 信号 | `GET /api/v1/signals` |
| 后验 | `GET /api/v1/performance?days=5&limit=200` |
| AI 用量 | `GET /api/v1/ai/usage?limit=20` |
| WS 统计 | `GET /api/v1/ws/stats` |
| AI 分析 | `POST /api/v1/ai/analyze/{code}` |
| WS | `ws://host:8787/ws` |

## 启动

```bash
# 1) 后端
cd ../../mj-server
cargo run

# 2) 静态壳
cd ..   # apps/
python3 -m http.server 8080
# 浏览器打开 http://127.0.0.1:8080/tauri_shell/
# 顶部 base URL 改成 http://127.0.0.1:8787，点「连接」
```
