一个**独立于 DeepSeek Harness 的开源项目**，而且第一阶段严格控制边界：**不做 Desktop App，不修改 DSH Web UI，只解决安装、运行、浏览器和进程管理。**

项目暂定名：

> **DSH Launcher**
> *A lightweight cross-platform installer and launcher for DeepSeek Harness.*

下面先确定整体技术方案，后面可以直接拆成连续的编程阶段交给 Coding Agent 实现。

## 1. 总体架构

```text
                    DSH Launcher
                         │
        ┌────────────────┼────────────────┐
        │                │                │
   Installation      Runtime          Browser
        │             Manager           Manager
        │                │                │
        ▼                ▼                ▼
   Node Runtime     DSH Process      Browser Discovery
   DSH Package      Start/Stop       Browser Selection
   Dependencies     Health Check     Remember Choice
                    Logs             Open URL
                         │
                         ▼
                  DeepSeek Harness
                         │
                         ▼
                   localhost:3080
                         │
                         ▼
                Chrome / Edge / Safari
```

核心原则：

**Launcher ≠ Harness**

Launcher 只是：

```text
Installer + Runtime Manager + Process Manager + Browser Manager
```

---

# 2. 技术栈

我建议第一版直接采用：

```text
Tauri 2
├── Rust
│   ├── Process Manager
│   ├── Runtime Manager
│   ├── Browser Manager
│   ├── Config Manager
│   ├── Health Checker
│   └── System Tray
│
└── Web UI
    └── React + Vite + TypeScript
```

这和你之前比较偏好的 React + Vite 技术路线也比较一致。

### 为什么不是 Electron？

这里并不需要 Electron 的能力。

我们的 UI 非常简单，而且核心工作其实是：

```text
启动进程
管理进程
检测端口
调用浏览器
读取配置
```

Tauri 更适合。

---

# 3. 项目结构

第一版我建议直接设计成：

```text
dsh-launcher/
│
├── src/
│   ├── components/
│   │   ├── StatusCard.tsx
│   │   ├── BrowserSelector.tsx
│   │   ├── RuntimeStatus.tsx
│   │   └── ActionButtons.tsx
│   │
│   ├── pages/
│   │   ├── Home.tsx
│   │   ├── BrowserSetup.tsx
│   │   └── Settings.tsx
│   │
│   ├── App.tsx
│   └── main.tsx
│
├── src-tauri/
│   ├── src/
│   │   ├── main.rs
│   │   │
│   │   ├── process/
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs
│   │   │   └── health.rs
│   │   │
│   │   ├── runtime/
│   │   │   ├── mod.rs
│   │   │   ├── detector.rs
│   │   │   └── installer.rs
│   │   │
│   │   ├── browser/
│   │   │   ├── mod.rs
│   │   │   ├── detector.rs
│   │   │   └── launcher.rs
│   │   │
│   │   ├── config/
│   │   │   ├── mod.rs
│   │   │   └── store.rs
│   │   │
│   │   └── tray.rs
│   │
│   ├── icons/
│   └── tauri.conf.json
│
├── scripts/
│   ├── build-runtime
│   └── package-dsh
│
├── package.json
├── README.md
└── LICENSE
```

这里特意把：

```text
process/
runtime/
browser/
config/
```

分开。

以后如果你把这个 Launcher 扩展到其他 Agent Runtime，不需要推翻架构。

---

# 4. Runtime 层

这是项目最重要的基础设施。

我们定义：

```rust
RuntimeManager
```

负责：

```text
detect()
install()
update()
remove()
version()
```

第一版目标：

```text
Node.js
DeepSeek Harness
```

---

## 4.1 Node.js

启动时：

```text
Detect Node.js
      │
      ├── Compatible
      │       ↓
      │     Use it
      │
      └── Missing / incompatible
              ↓
        Use bundled Node.js
```

注意：

**不强制修改用户系统的 Node.js。**

我们优先使用自己的 runtime：

```text
~/.dsh-launcher/runtime/
```

或者平台对应的 App Data 目录。

---

# 5. DSH 安装

Launcher 不应该要求：

```bash
npm install
```

而应该自动完成。

逻辑：

```text
Check DSH
   │
   ├── Installed
   │      ↓
   │    Check version
   │
   └── Missing
          ↓
     Install DSH
```

以后：

```text
Update available
       ↓
[ Update ]
```

---

# 6. Process Manager

这是第二个核心模块。

定义统一接口：

```text
start()
stop()
restart()
status()
pid()
logs()
```

状态：

```text
Starting
Running
Stopping
Stopped
Error
```

内部流程：

```text
start()
   ↓
spawn DSH process
   ↓
capture stdout/stderr
   ↓
monitor process
   ↓
health check
   ↓
Running
```

---

# 7. 不使用 sleep 判断启动完成

这一点我们从一开始就定死。

不能：

```text
start()
sleep(3)
openBrowser()
```

必须：

```text
start()
   ↓
poll http://127.0.0.1:3080
   ↓
HTTP success
   ↓
Running
   ↓
openBrowser()
```

这样不同机器上的体验才稳定。

---

# 8. Browser Manager

这就是你刚刚提出的核心改进。

定义：

```text
BrowserManager

detect()
list()
getDefault()
setDefault()
open()
```

启动时：

```text
Stored Browser?
       │
       ├── Yes
       │    ↓
       │  Launch
       │
       └── No
            ↓
      Browser Selection
```

---

# 9. Browser Selection

第一次：

```text
Choose your browser

○ Google Chrome
○ Microsoft Edge
○ Firefox
○ Safari
○ Brave

☑ Remember my choice

             [ Continue ]
```

检测结果决定显示哪些浏览器。

例如 Windows：

```text
✓ Chrome
✓ Edge
✓ Firefox
```

macOS：

```text
✓ Safari
✓ Chrome
✓ Edge
```

---

# 10. Browser Preference

配置文件：

```json
{
  "browser": {
    "type": "chrome",
    "remember": true
  },
  "server": {
    "host": "127.0.0.1",
    "port": 3080
  }
}
```

以后：

```text
double click
    ↓
start DSH
    ↓
health check
    ↓
Chrome
    ↓
open http://127.0.0.1:3080
```

---

# 11. Launcher UI

主窗口建议非常克制。

```text
┌─────────────────────────────────────┐
│                                     │
│              [LOGO]                 │
│                                     │
│          DSH Launcher               │
│                                     │
│          ● Running                  │
│                                     │
│   DeepSeek Harness is ready.        │
│                                     │
│       ┌───────────────────┐         │
│       │   Open Harness    │         │
│       └───────────────────┘         │
│                                     │
│       Restart        Stop           │
│                                     │
│ ─────────────────────────────────── │
│                                     │
│  Harness       Running              │
│  Browser       Chrome               │
│  Port          3080                 │
│                                     │
│                       ⚙ Settings    │
└─────────────────────────────────────┘
```

品牌部分完全属于你的项目。

---

# 12. System Tray / Menu Bar

窗口关闭以后：

**不是退出。**

而是：

```text
Window
   ↓
close
   ↓
hide
   ↓
Tray
```

Windows：

```text
System Tray
   [DSH]
```

macOS：

```text
Menu Bar
   [DSH]
```

菜单：

```text
DSH Launcher
────────────────
● Running

Open Harness
Restart
Stop
────────────────
Browser: Chrome
────────────────
Settings
View Logs
Quit
```

---

# 13. 关闭行为

这里建议明确：

### 点击窗口关闭

```text
隐藏 Launcher
DSH 继续运行
```

### Tray → Quit

```text
Stop DSH
退出 Launcher
```

这样非常符合服务器型应用的习惯。

---

# 14. 自动启动

第一版可以支持：

```text
☐ Launch DSH Launcher at system startup
```

如果开启：

```text
Windows/macOS login
        ↓
DSH Launcher
        ↓
Start DSH
        ↓
Tray
```

但：

**不自动打开浏览器。**

这是很重要的体验区别。

开机启动：

```text
启动服务
```

手动双击：

```text
启动服务
+
打开浏览器
```

---

# 15. 错误处理

用户永远不应该看到：

```text
panic
stack trace
npm error
node error
```

而应该看到：

```text
Unable to start DeepSeek Harness

Something went wrong while starting the service.

[ Retry ]

Show Details
```

Details：

```text
Error: Port 3080 is already in use.
```

如果是端口冲突：

```text
Port 3080 is already in use.

[ Use Another Port ]

Available:
3081
3082
3083
```

---

# 16. 日志

Launcher 自己保存：

```text
logs/
    launcher.log
    harness.log
```

UI：

```text
Settings
   ↓
Diagnostics
   ↓
View Logs
```

这对开源项目后期处理用户 Issue 非常重要。

---

# 17. Installer

最终我们需要：

### Windows

```text
DSH-Launcher-Setup.exe
```

安装：

```text
C:\Users\<user>\AppData\Local\DSH Launcher\
```

并创建：

```text
Desktop shortcut
Start Menu
```

### macOS

```text
DSH-Launcher.dmg
```

安装：

```text
/Applications/DSH Launcher.app
```

---

# 18. 最终用户体验

我们最终追求的其实就是：

### 第一次

```text
下载
 ↓
安装
 ↓
双击
 ↓
环境自动准备
 ↓
选择浏览器
 ↓
记住选择
 ↓
启动 DSH
 ↓
打开浏览器
```

### 第二次

```text
双击
 ↓
DSH 启动
 ↓
Chrome 自动打开
```

### 第三次以后

甚至可以：

```text
点击托盘图标
 ↓
Open Harness
```

---

# 19. 开发阶段我建议严格拆成 5 个 Stage

为了后面让 Coding Agent 连续开发，我建议不要一次把整个项目扔给它。

### Stage 1 — Foundation

```text
Tauri 2
React + Vite
项目结构
跨平台基础
品牌 UI
配置系统
```

### Stage 2 — DSH Runtime

```text
Runtime detection
Node runtime
DSH installation
DSH version
Process Manager
Start / Stop / Restart
```

### Stage 3 — Browser

```text
Browser detection
Browser selection
Remember choice
Browser launching
Health Check
```

### Stage 4 — System Integration

```text
Windows Tray
macOS Menu Bar
Auto Start
Notifications
Close → Tray
Quit → Stop
```

### Stage 5 — Distribution

```text
Windows Installer
macOS DMG
Code signing
Auto Update
Release workflow
GitHub Actions
```

---

## 20. 我建议第一版的项目定位

README 首页甚至可以非常简单：

> **DSH Launcher**
>
> A lightweight cross-platform installer and launcher for DeepSeek Harness.
>
> Install once. Double-click to start. Use DeepSeek Harness directly in your browser.

下面明确：

```text
✓ Windows
✓ macOS
✓ One-click installation
✓ Automatic DSH startup
✓ Browser selection
✓ Remember browser preference
✓ System tray / menu bar
✓ Start / Stop / Restart
✓ No command line required
```

以及最关键的一句：

> **DSH Launcher does not replace the DeepSeek Harness web interface. It simply makes it easier to install and run.**

这个定位我认为非常好。

---

**下一步就可以开始实际编程了。** 我建议我们按照上面的 5 个 Stage 一次做一个，**先做 Stage 1**：建立 Tauri 2 + React/Vite/TypeScript 项目骨架、品牌化 Launcher 主窗口、配置系统和跨平台抽象，同时暂时不实现 DSH 安装/启动。这样 Stage 2 接上 Runtime 和 Process Manager 时架构会比较干净。
