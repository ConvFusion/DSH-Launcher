# DSH Launcher

**DeepSeek Harness 的一键安装与启动器（Installer & Launcher）——面向完全不会使用命令行的用户。**

一次安装，双击启动，直接在浏览器里使用 DeepSeek Harness。不需要安装 Node.js，不需要打开命令行，不需要运行 `npx @deepseek-ai/dsh web`。

```text
✓ 支持 Windows 与 macOS
✓ 一键安装（自动装好 Node.js 运行时 + DeepSeek Harness）
✓ 首页只有 LOGO 和 一个按钮
     未安装 → 「安装 DeepSeek Harness (v版本号)」
     已安装 → 「打开 DeepSeek Harness (v版本号)」（未运行会自动先启动）
✓ 版本号检测自 npm 上 @deepseek-ai/dsh 的最新版（即 npx 实际会安装的版本）
✓ 检测到 npm 有新版本时，首页「打开」按钮旁显示小「更新到 v版本号」按钮，一键更新
✓ 系统托盘 / 菜单栏（打开、重启、停止、设置、退出）
```

> **DSH Launcher 并不替代 DeepSeek Harness 的 Web 界面，它只是让安装和运行变得更简单。**

设置页只保留小白用户能理解的两项：语言（中文 / English）与主题（跟随系统 / 浅色 / 深色）。
服务器地址、端口、Node.js 运行时、安装目录、浏览器选择、日志等高级配置已全部移除。

---

## 工作原理（How it works）

```text
双击启动
    │
    ▼
已在运行？  ── 是 ──▶ 接管并显示"运行中"（也能检测到手动启动的实例）
    │
    ▼
未安装 ──▶ 点击「安装」→ 自动下载内置 Node.js + npm 安装 @deepseek-ai/dsh
    │
    ▼
启动 `dsh web --host 127.0.0.1 --port 3080 --no-open`
    │
    ▼
健康检查 ── 轮询 http://127.0.0.1:3080 直到收到 HTTP 响应（最多 120 秒）
    │
    ▼
打开系统默认浏览器（不再询问选择哪个浏览器）
    │
    ▼
关闭窗口 → 隐藏到托盘，DSH 继续运行
托盘 → 退出 → 停止 DSH 并退出
```

## 开发（Development）

```bash
npm install
npm run build        # 前端（tsc + vite）
bash scripts/with-toolchain.sh cargo check --manifest-path src-tauri/Cargo.toml   # 后端检查
npm run tauri dev    # 本地调试（需要 Rust toolchain）
```
