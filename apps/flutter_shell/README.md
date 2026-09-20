# flutter_shell

Flutter 端占位壳 — 对接后端 HTTP + WS。

## 文件

- `lib/main.dart` — 入口
- `lib/mojin_models.dart` — 与 `mj-server` JSON 对齐的实体（Healthz/Account/Position/SignalRow/Performance）
- `lib/mojin_api.dart` — `MojinApi`（HTTP + WebSocket 客户端）
- `lib/home_screen.dart` — 命令式复盘工作台（总览/账户/信号/后验/AI/WS 日志）

## 后端约定

| 项 | 值 |
| --- | --- |
| 端口 | `8787` |
| Healthz | `GET /healthz` |
| 账户 | `GET /api/v1/paper/account` |
| 信号 | `GET /api/v1/signals` |
| 后验 | `GET /api/v1/performance?days=5&limit=200` |
| WS | `ws://host:8787/ws` |

订阅示例：

```dart
api.wsStream(['signal', 'fill', 'equity', 'healthz', 'ai']).listen((ev) {
  print('${ev.ts} ${ev.topic} ${ev.payload}');
});
```

## 运行

```bash
flutter pub get
flutter run \
  --dart-define=MOJIN_BASE=http://192.168.1.10:8787
```