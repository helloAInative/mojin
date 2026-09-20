# 摸金小王子（Mojin）

[![CI](https://github.com/helloAInative/mojin/actions/workflows/ci.yml/badge.svg)](https://github.com/helloAInative/mojin/actions/workflows/ci.yml)
[![Desktop](https://github.com/helloAInative/mojin/actions/workflows/desktop.yml/badge.svg)](https://github.com/helloAInative/mojin/actions/workflows/desktop.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

面向家庭自用场景的 A 股行情、技术信号、模拟交易和复盘服务。后端使用 Rust、Axum 与 SQLite，附带基于 Tauri 2 的 macOS/Windows 客户端、零构建 Web 工作台和 Flutter 参考客户端。

> 本项目不连接券商、不执行真实交易、不代客理财。行情和模型输出可能延迟或出错，所有内容仅供软件研究与纸上统计，不构成投资建议。

## 功能

- 腾讯优先，东财与新浪备用的实时行情；腾讯与东财前复权日线
- MACD、KDJ、RSI 与量能信号，包含等级、置信度和触发因子
- 1/3/5/10/20 日信号后验及命中率统计
- 模拟撮合：整手买入、T+1 可卖、涨跌停、佣金、印花税和过户费
- 持仓行情标记、每日权益快照及相邻快照收益
- 自选组和指数池维护
- 新闻、昨日美股、行业板块、技术指标与历史后验驱动的研究候选池
- 五角色 OpenAI 兼容模型并行分析、加权仲裁、用量记录与逐路本地降级
- WebSocket 主题订阅：`signal`、`fill`、`equity`、`healthz`、`ai`
- macOS 与 Windows 桌面客户端，支持服务地址记忆和断线重连

## 项目状态

项目处于早期开发阶段，适合本地研究和二次开发。配置服务端 Token 后会调用 OpenAI 兼容接口；没有 Token 或单路调用失败时使用确定性的本地启发式。历史信号后验作为研究证据参与分析，不会自动训练或微调外部模型。

已知限制：

- T+1 按北京时间自然日切换，尚未接入交易所休市日历。
- 行情来自公开接口，未提供可用性或实时性保证。
- 未标记行情的持仓按成本估值，响应中的 `unpriced_positions` 会给出数量。
- `apps/flutter_shell` 是客户端参考源码，尚未包含完整平台工程。

## 快速开始

需要 Rust 1.88 或更高版本。

```bash
git clone git@github.com:helloAInative/mojin.git
cd mojin
./scripts/dev-up.sh --fg
```

另开终端检查服务：

```bash
curl http://127.0.0.1:8787/healthz
curl http://127.0.0.1:8787/api/v1/meta
```

启动 Web 工作台：

```bash
cd apps
python3 -m http.server 8080
```

浏览器访问 `http://127.0.0.1:8080/tauri_shell/`。

### macOS / Windows 客户端

```bash
cd apps/tauri_shell
npm install
npm run desktop:dev
```

生成当前平台安装包：

```bash
npm run desktop:build
```

详细环境要求和产物路径见 [桌面客户端文档](apps/tauri_shell/README.md)。

## Docker Compose

```bash
cp .env.example .env
docker compose build
docker compose up -d
curl http://127.0.0.1:8787/healthz
```

服务默认只绑定宿主机 `127.0.0.1`，SQLite 数据保存在 `./data`。如需在家庭网络访问，请自行配置可信反向代理或私有网络，不建议直接暴露到公网。

## 配置

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `MJ_HOST` | `0.0.0.0` | 服务监听地址；开发脚本会改为 `127.0.0.1` |
| `MJ_PORT` | `8787` | HTTP 与 WebSocket 端口 |
| `MJ_DB_PATH` | `data/mojin.db` | SQLite 路径 |
| `RUST_LOG` | `mj_server=info,tower_http=info` | Rust 日志过滤器 |
| `MJ_AI_API_KEY` | 空 | OpenAI 兼容接口 Token；只在服务端读取 |
| `MJ_AI_BASE_URL` | OpenAI | 兼容接口基址，例如阿里百炼 `/compatible-mode/v1` |
| `MJ_AI_MODEL` | `gpt-5-mini` | 五角色默认模型 |
| `MJ_AI_MODEL_{ROLE}` | 空 | 按角色覆盖模型，角色见 `.env.example` |
| `MJ_AI_TIMEOUT_SECS` | `45` | 单路模型超时，范围 5–180 秒 |
| `MJ_NEWS_FEEDS` | 空 | 自定义 RSS，格式为 `来源|URL;来源|URL` |

真实密钥只能写入未提交的 `.env`，不要写入源码、Issue 或日志。

配置真实 AI：

```bash
cp .env.example .env
# 编辑 .env，填写 MJ_AI_API_KEY、MJ_AI_BASE_URL、MJ_AI_MODEL
./scripts/dev-up.sh --rebuild
curl http://127.0.0.1:8787/api/v1/ai/config
```

真实模型会收到候选股票的行情、技术因子、历史后验、新闻标题、美股和板块摘要。不要在自定义新闻或股票备注中放入个人信息或账户凭据。

默认研究数据来自东方财富聚合新闻、腾讯美股行情和新浪行业板块；响应保留来源、采集时间与交易日期。任一来源失败时会在 `context.errors` 中明确返回，不会用估算值填补。

## 常用操作

```bash
./scripts/dev-up.sh          # 后台启动
./scripts/dev-log.sh         # 查看日志
./scripts/dev-down.sh        # 停止服务
./scripts/backup-db.sh       # 一致性备份 SQLite

cd mj-server
cargo test --offline         # 使用已有依赖运行测试
cargo test                   # 正常联网解析依赖
```

AI 分析可基于当前行情，也可绑定历史信号：

```bash
curl -X POST http://127.0.0.1:8787/api/v1/ai/analyze/sz300623 \
  -H 'Content-Type: application/json' \
  -d '{"signal_id":"信号 ID"}'
```

传入 `signal_id` 时，服务会校验股票代码，并使用该信号落库时的价格、置信度、因子和后验。分析完成后会广播 `ai` WebSocket 事件。

## 目录

```text
mojin/
├── mj-server/          # Rust API、行情、信号、模拟盘和 WebSocket
├── apps/
│   ├── tauri_shell/    # Tauri 2 macOS/Windows 客户端与共享 Web 工作台
│   └── flutter_shell/  # Flutter 参考客户端源码
├── docs/               # 架构和 API 文档
├── scripts/            # 开发、日志和备份脚本
└── docker-compose.yml
```

详细设计见 [架构文档](docs/ARCHITECTURE.md)，端点见 [API 文档](docs/API.md)。

## 参与贡献

欢迎提交 Issue 和 Pull Request。开始前请阅读 [贡献指南](CONTRIBUTING.md)、[行为准则](CODE_OF_CONDUCT.md)与[安全策略](SECURITY.md)。

## 许可证

[MIT](LICENSE) © 2026 helloAInative
