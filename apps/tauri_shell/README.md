# Mojin Desktop

基于 Tauri 2 的摸金小王子桌面客户端，同一套代码支持 macOS 和 Windows。客户端内嵌现有复盘工作台，通过 HTTP 和 WebSocket 连接 `mj-server`。

## 当前能力

- 总览、模拟账户、持仓估值、信号、后验、AI 仲裁和 WebSocket 日志
- 新闻、昨日美股、行业板块、历史后验和技术指标驱动的智能候选池
- 服务端可配置五路 OpenAI 兼容模型，单路失败自动使用本地启发式
- 服务地址本地保存、格式校验与 WebSocket 断线重连
- macOS `.app` / `.dmg` 和 Windows NSIS `.exe` 打包配置
- 最小 Tauri 权限：主窗口只有 `core:default`，没有文件、Shell 或系统命令权限
- GitHub Actions 在 macOS 和 Windows 上执行原生编译检查

## 开发

先启动后端：

```bash
./scripts/dev-up.sh
```

再启动桌面客户端：

```bash
cd apps/tauri_shell
npm install
npm run desktop:dev
```

macOS 需要 Xcode Command Line Tools。Windows 需要 Microsoft C++ Build Tools 的“使用 C++ 的桌面开发”工作负载；Windows 10 1803 及更高版本通常已经包含 WebView2。

## 检查与打包

```bash
npm run check
npm run desktop:build
```

默认产物位置：

- macOS：`src-tauri/target/release/bundle/macos/` 与 `dmg/`
- Windows：`src-tauri/target/release/bundle/nsis/`

当前安装包只包含客户端。运行客户端前需单独启动 `mj-server`，默认地址为 `http://127.0.0.1:8787`。顶部输入框可以连接局域网中的服务，地址会保存在本机 WebView 存储中。

真实 AI 的 Token 只配置在项目根目录未提交的 `.env` 中。客户端通过 `/api/v1/ai/config` 显示启用状态，不会读取或展示 Token。

## 浏览器预览

桌面 UI 仍可作为普通静态页面预览：

```bash
cd apps
python3 -m http.server 8080
```

打开 `http://127.0.0.1:8080/tauri_shell/`。

## 目录

```text
tauri_shell/
├── index.html            # 共享 UI
├── app-icon.svg          # 应用图标源文件
├── scripts/build.mjs     # 静态资源构建
├── package.json          # Tauri CLI 与桌面命令
└── src-tauri/
    ├── capabilities/     # 桌面权限边界
    ├── icons/            # macOS/Windows 图标
    ├── src/              # 原生入口
    └── tauri.conf.json   # 窗口、安全与安装包配置
```
