# DSH Launcher

> **DeepSeek Harness 的一键安装与启动器（Installer & Launcher）—— 面向完全不会使用命令行的用户。**
>
> A lightweight cross-platform installer and launcher for **DeepSeek Harness**. Double-click to start, use DSH directly in your browser — no Node.js, no terminal, no `npx`.

---

<p align="center">
  <b>English</b> &nbsp;|&nbsp; <a href="#-简体中文">简体中文</a>
</p>

---

## ✨ Features

| | Feature | Description |
|---|---------|-------------|
| 🖱️ | **One-click install** | Automatically downloads and installs Node.js (SHA-256 verified, from nodejs.org) and DeepSeek Harness (from npm) — zero configuration. |
| 🚀 | **One-click launch** | A single big button on the home screen: "Install DeepSeek Harness" or "Open DeepSeek Harness" depending on state. |
| 🔄 | **Auto-update detection** | Checks npm for the latest `@deepseek-ai/dsh` version and offers a one-click update button when a new release is available. |
| 📊 | **Health-checked startup** | Polls the DSH HTTP endpoint until it responds (up to 120s) — no fixed sleep guesses. |
| 🧩 | **External instance adoption** | If you already have DSH running (e.g. via `npx @deepseek-ai/dsh web`), the launcher detects and adopts it. |
| 🔔 | **System tray / menu bar** | Open / Restart / Stop / Settings / Quit — all from the tray icon. Closing the window hides to the tray; DSH keeps running. |
| 🌐 | **Dual language** | English & 简体中文 interface. |
| 🎨 | **Theme support** | Follow system / Light / Dark. |
| 🔑 | **Launch at login** | Optional autostart — starts DSH silently in the background on OS login. |
| 🛡️ | **Sandboxed install** | Everything goes into `~/.dsh-launcher/`. Your system Node.js is never touched. |

---

## 🚀 Quick Start

### Download

Grab the latest build from the [Releases](https://github.com/ConvFusion/DSH-Launcher/releases) page:

- **macOS**: `.dmg` (Apple Silicon & Intel)
- **Windows**: `.exe` installer (NSIS, per-user install)

### Install & Run

1. Open the installer / drag to Applications.
2. Launch **DSH Launcher**.
3. Click **Install DeepSeek Harness** (first run only — downloads Node.js + DSH).
4. Click **Open DeepSeek Harness** — it opens in your default browser.
5. Close the window to hide to the system tray. DSH continues running.

> ⚠️ **macOS says “DSH Launcher” is damaged and can’t be opened?**
>
> That is Gatekeeper blocking an unsigned / not-notarized build — **the file is
> not actually damaged**. Two ways to open it:
>
> **Option A — right-click to open (one-time):**
> 1. In **Finder → Applications**, right-click (or Control-click) **DSH Launcher**.
> 2. Click **Open**, then click **Open** again in the confirmation dialog.
>
> **Option B — remove the quarantine flag (permanent for this copy):**
> ```bash
> xattr -cr "/Applications/DSH Launcher.app"
> ```

---

## 🧠 How It Works

```
Double-click launcher
     │
     ▼
Already running?  ── Yes ──▶ Show "Running" (adopts manual instances too)
     │
     ▼
Not installed ──▶ Click "Install" → download Node.js + npm install @deepseek-ai/dsh
     │
     ▼
Start  `node <dsh>/lib/bin.js web --host 127.0.0.1 --port 3080 --no-open`
     │
     ▼
Health check  ──  poll http://127.0.0.1:3080 until HTTP OK (max 120s)
     │
     ▼
Open default browser
     │
     ▼
Close window → hide to tray, DSH keeps running
Tray → Quit  → stop DSH and exit
```

### Data Directory

All files are stored under `~/.dsh-launcher/`:

```
~/.dsh-launcher/
├── config.json      # User preferences (language, theme, server host/port, …)
├── state.json       # PID / port of the last managed DSH process
├── logs/
│   ├── launcher.log # Launcher debug log
│   └── harness.log  # DSH process stdout/stderr (rotated at 5 MB)
├── runtime/         # Bundled Node.js (installed on demand, SHA-256 verified)
└── dsh/             # Managed DeepSeek Harness installation (npm install)
```

---

## 🛠️ Development

### Prerequisites

- **Node.js** ≥ 20 (LTS recommended)
- **Rust** stable (edition 2021, MSRV 1.77.2)
- **Tauri 2** prerequisites:
  - **macOS**: Xcode Command Line Tools
  - **Windows**: WebView2 + Visual Studio Build Tools

### Setup

```bash
# Install dependencies
npm install

# Frontend dev server (runs on :1420)
npm run dev

# Rust-only check
cargo check --manifest-path src-tauri/Cargo.toml

# Full Tauri dev (frontend + backend, opens the app window)
npm run tauri dev
```

### Scripts

| Script | Description |
|--------|-------------|
| `npm run dev` | Start Vite dev server (frontend only) |
| `npm run build` | Type-check + build frontend bundle to `dist/` |
| `npm run preview` | Preview the built frontend |
| `npm run tauri` | Run Tauri CLI (uses the project-local toolchain if present) |
| `npm run tauri dev` | Full dev mode with live reload |
| `npm run tauri build` | Produce release bundles (`.dmg` / `.exe`) |

### Project Structure

```
.
├── src/                     # Frontend (React + TypeScript)
│   ├── App.tsx              # Main app shell, status state, theme/i18n wiring
│   ├── api.ts               # Typed Tauri IPC command wrappers
│   ├── types.ts             # Shared types mirroring the Rust backend
│   ├── pages/
│   │   ├── Home.tsx         # One big button + update + URL copy
│   │   └── Settings.tsx     # Language, theme, about
│   ├── i18n/                # en / zh translation system
│   ├── components/Logo.tsx
│   └── styles.css
├── src-tauri/               # Backend (Rust + Tauri 2)
│   ├── src/
│   │   ├── main.rs          # Entry point
│   │   ├── lib.rs           # Tauri builder, plugins, setup
│   │   ├── commands.rs      # IPC command surface
│   │   ├── state.rs         # Shared app state, startup orchestration
│   │   ├── tray.rs          # System tray menu
│   │   ├── config/          # Config store, data dir, logging
│   │   ├── process/         # DSH process manager + health checks
│   │   ├── runtime/         # Node.js & DSH detection + installer
│   │   └── browser/         # Browser detection & launching
│   ├── Cargo.toml
│   └── tauri.conf.json
├── scripts/                 # Build / sign / toolchain helper scripts
└── public/                  # Static assets (icons)
```

### Architecture Overview

```
┌──────────────────────────────────────────────────────┐
│  Frontend (React + TypeScript, Vite)                 │
│  ┌─────────┐  ┌──────────┐  ┌──────────────────┐    │
│  │  Home   │  │ Settings │  │  i18n / Theme     │    │
│  └────┬────┘  └────┬─────┘  └────────┬─────────┘    │
│       └──────┬──────┘                │              │
│              ▼                       ▼              │
│         api.ts (IPC)            App.tsx state       │
└──────────────┬───────────────────────┬──────────────┘
               │  Tauri invoke / emit  │
┌──────────────▼───────────────────────▼──────────────┐
│  Backend (Rust + Tokio + Tauri 2)                    │
│  ┌───────────┐  ┌───────────┐  ┌────────────────┐  │
│  │  process  │  │  runtime  │  │  config / tray │  │
│  │  manager  │  │  install  │  │                │  │
│  └─────┬─────┘  └─────┬─────┘  └───────┬────────┘  │
│        └────────┬─────┘                │           │
│                 ▼                      ▼           │
│           AppState (Mutex)        Commands (IPC)    │
└──────────────────────────────────────────────────────┘
```

---

## 🏗️ Build Release

```bash
# Produce signed/unsigned bundles for the current platform
npm run tauri build

# Output:
#   macOS: src-tauri/target/release/bundle/dmg/*.dmg
#   Windows: src-tauri/target/release/bundle/nsis/*.exe
```

The GitHub Actions **Release** workflow (`.github/workflows/release.yml`) automates this:
- Triggers on `v*` tags
- Builds macOS (aarch64 + x86_64) and Windows (x86_64)
- Supports optional code signing (Apple certificate + notarization, Windows certificate)

---

## CI

| Workflow | Trigger | What it does |
|----------|---------|--------------|
| `ci.yml` | push to `main` / PRs | Frontend type-check + build + Rust check on macOS & Windows |
| `release.yml` | `v*` tags / manual | Full Tauri build + GitHub Release draft |

---

## 📝 License

Apache-2.0 © DeepSeek Harness contributors.

---

---

# <a id="zh"></a>📖 简体中文

## ✨ 功能特性

| | 功能 | 说明 |
|---|------|------|
| 🖱️ | **一键安装** | 自动下载安装 Node.js（SHA-256 校验，来自 nodejs.org）和 DeepSeek Harness（来自 npm）—— 零配置。 |
| 🚀 | **一键启动** | 首页只有一个大按钮：未安装时显示「安装 DeepSeek Harness」，已安装时显示「打开 DeepSeek Harness」。 |
| 🔄 | **自动更新检测** | 检测 npm 上 `@deepseek-ai/dsh` 的最新版本，有新版本时显示一键更新按钮。 |
| 📊 | **健康检查启动** | 轮询 DSH HTTP 接口直到响应（最长 120 秒）—— 不靠固定 sleep 猜时间。 |
| 🧩 | **接管外部实例** | 如果你已经通过 `npx @deepseek-ai/dsh web` 等方式运行了 DSH，启动器会自动检测并接管。 |
| 🔔 | **系统托盘 / 菜单栏** | 打开 / 重启 / 停止 / 设置 / 退出 —— 全部从托盘图标操作。关闭窗口隐藏到托盘，DSH 继续运行。 |
| 🌐 | **双语界面** | 支持 English 和 简体中文。 |
| 🎨 | **主题切换** | 跟随系统 / 浅色 / 深色。 |
| 🔑 | **开机自启** | 可选开机自启 —— 登录时静默启动 DSH（不打开浏览器）。 |
| 🛡️ | **沙箱式安装** | 所有文件放在 `~/.dsh-launcher/`，绝不修改系统 Node.js。 |

---

## 🚀 快速开始

### 下载

从 [Releases](https://github.com/ConvFusion/DSH-Launcher/releases) 页面下载最新版本：

- **macOS**: `.dmg`（Apple Silicon & Intel）
- **Windows**: `.exe` 安装包（NSIS，当前用户安装）

### 安装与运行

1. 打开安装包 / 拖到应用程序。
2. 启动 **DSH Launcher**。
3. 点击 **安装 DeepSeek Harness**（仅首次运行 —— 下载 Node.js + DSH）。
4. 点击 **打开 DeepSeek Harness** —— 在默认浏览器中打开。
5. 关闭窗口隐藏到系统托盘，DSH 继续在后台运行。

> ⚠️ **macOS 提示「"DSH Launcher"已损坏，无法打开」？**
>
> 这是 Gatekeeper 拦截了未签名 / 未公证的版本——**文件并没有真的损坏**。两种打开方式：
>
> **方法一：右键打开（一次性）**
> 1. 在 **访达 → 应用程序** 中，右键（或按住 Control 点击）**DSH Launcher**。
> 2. 点击 **打开**，在弹出的确认对话框中再次点击 **打开**。
>
> **方法二：移除隔离属性（对该副本永久生效）**
> ```bash
> xattr -cr "/Applications/DSH Launcher.app"
> ```

---

## 🧠 工作原理

```
双击启动
    │
    ▼
已在运行？  ── 是 ──▶ 显示"运行中"（也能接管手动启动的实例）
    │
    ▼
未安装 ──▶ 点击「安装」→ 下载 Node.js + npm 安装 @deepseek-ai/dsh
    │
    ▼
启动 `node <dsh>/lib/bin.js web --host 127.0.0.1 --port 3080 --no-open`
    │
    ▼
健康检查 ── 轮询 http://127.0.0.1:3080 直到响应（最长 120 秒）
    │
    ▼
打开默认浏览器
    │
    ▼
关闭窗口 → 隐藏到托盘，DSH 继续运行
托盘 → 退出 → 停止 DSH 并退出
```

### 数据目录

所有文件都存放在 `~/.dsh-launcher/` 下：

```
~/.dsh-launcher/
├── config.json      # 用户偏好设置（语言、主题、服务器地址/端口等）
├── state.json       # 上一次启动的 DSH 进程的 PID / 端口
├── logs/
│   ├── launcher.log # 启动器调试日志
│   └── harness.log  # DSH 进程标准输出/错误（5 MB 滚动）
├── runtime/         # 捆绑的 Node.js（按需安装，SHA-256 校验）
└── dsh/             # 受管理的 DeepSeek Harness 安装（npm install）
```

---

## 🛠️ 开发

### 前置条件

- **Node.js** ≥ 20（推荐 LTS）
- **Rust** stable（edition 2021，MSRV 1.77.2）
- **Tauri 2** 前置依赖：
  - **macOS**：Xcode Command Line Tools
  - **Windows**：WebView2 + Visual Studio Build Tools

### 开始开发

```bash
# 安装依赖
npm install

# 前端开发服务器（运行在 :1420）
npm run dev

# 仅检查 Rust 代码
cargo check --manifest-path src-tauri/Cargo.toml

# 完整 Tauri 开发模式（前端 + 后端，打开应用窗口）
npm run tauri dev
```

### 构建脚本

| 脚本 | 说明 |
|------|------|
| `npm run dev` | 启动 Vite 开发服务器（仅前端） |
| `npm run build` | 类型检查 + 构建前端到 `dist/` |
| `npm run preview` | 预览构建好的前端 |
| `npm run tauri` | 运行 Tauri CLI（如存在则使用项目本地工具链） |
| `npm run tauri dev` | 完整开发模式，带热重载 |
| `npm run tauri build` | 生成发布包（`.dmg` / `.exe`） |

### 项目结构

```
.
├── src/                     # 前端（React + TypeScript）
│   ├── App.tsx              # 主应用外壳、状态管理、主题/国际化
│   ├── api.ts               # 类型化的 Tauri IPC 命令封装
│   ├── types.ts             # 与 Rust 后端对应的共享类型
│   ├── pages/
│   │   ├── Home.tsx         # 首页：大按钮 + 更新 + 复制URL
│   │   └── Settings.tsx     # 设置页：语言、主题、关于
│   ├── i18n/                # 英/中文翻译系统
│   ├── components/Logo.tsx
│   └── styles.css
├── src-tauri/               # 后端（Rust + Tauri 2）
│   ├── src/
│   │   ├── main.rs          # 入口点
│   │   ├── lib.rs           # Tauri 构建器、插件、初始化
│   │   ├── commands.rs      # IPC 命令接口
│   │   ├── state.rs         # 应用共享状态、启动编排
│   │   ├── tray.rs          # 系统托盘菜单
│   │   ├── config/          # 配置存储、数据目录、日志
│   │   ├── process/         # DSH 进程管理 + 健康检查
│   │   ├── runtime/         # Node.js & DSH 检测 + 安装
│   │   └── browser/         # 浏览器检测与启动
│   ├── Cargo.toml
│   └── tauri.conf.json
├── scripts/                 # 构建 / 签名 / 工具链辅助脚本
└── public/                  # 静态资源（图标）
```

---

## 🏗️ 构建发布版本

```bash
# 为当前平台生成签名/未签名的发布包
npm run tauri build

# 输出：
#   macOS: src-tauri/target/release/bundle/dmg/*.dmg
#   Windows: src-tauri/target/release/bundle/nsis/*.exe
```

GitHub Actions **Release** 工作流（`.github/workflows/release.yml`）会自动执行：
- 触发条件：推送 `v*` 标签
- 构建 macOS（aarch64 + x86_64）和 Windows（x86_64）
- 支持可选代码签名（Apple 证书 + 公证、Windows 证书）

---

## 持续集成

| 工作流 | 触发条件 | 内容 |
|--------|---------|------|
| `ci.yml` | 推送到 `main` / 提交 PR | 前端类型检查 + 构建 + Rust 检查（macOS & Windows） |
| `release.yml` | `v*` 标签 / 手动触发 | 完整 Tauri 构建 + GitHub Release 发布 |

---

## 📝 许可证

Apache-2.0 © DeepSeek Harness 贡献者。
