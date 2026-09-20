# 更新记录

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构，并计划在稳定发布后采用语义化版本。

## Unreleased

### Added

- Rust/Axum API、SQLite 数据模型与 WebSocket 主题订阅。
- 多来源 A 股行情、技术信号和多窗口后验。
- 模拟撮合、持仓估值和每日权益快照。
- 五角色本地启发式分析与 AI 用量记录。
- 静态 Web 工作台和 Flutter 参考客户端。

### Fixed

- 模拟账户现金、冻结资金、权益和 T+1 可卖数量的一致性。
- 持仓快照的事务回滚、行情时间保留和备源优先级。
- WebSocket 客户端订阅互相覆盖的问题。
