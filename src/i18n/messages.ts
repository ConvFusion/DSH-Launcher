// Translation dictionaries. The `en` dictionary defines the canonical keys;
// `zh` is type-checked to have exactly the same keys.

export type Language = "en" | "zh";

const en = {
  // App shell
  "app.loading": "Starting…",
  "app.settings": "Settings",
  "app.close_settings": "Back",
  "app.plugins": "Plugins",
  "app.close_plugins": "Back",

  // Home — one big button
  "home.install": "Install DeepSeek Harness",
  "home.install_v": "Install DeepSeek Harness (v{version})",
  "home.open": "Open DeepSeek Harness",
  "home.open_v": "Open DeepSeek Harness (v{version})",
  "home.busy.installing": "Installing…",
  "home.busy.opening": "Opening…",
  "home.busy.updating": "Updating…",
  "home.busy.starting": "Starting…",
  "home.busy.stopping": "Stopping…",
  "home.busy.restarting": "Restarting…",
  "home.update": "Update",
  "home.update_v": "Update to v{version}",
  "home.updated": "DeepSeek Harness updated to v{version}.",
  "home.copy_url": "Copy URL",
  "home.start": "Start",
  "home.stop": "Stop",
  "home.restart": "Restart",

  // Status dots
  "status.stopped": "Stopped",
  "status.starting": "Starting…",
  "status.running": "Running",
  "status.stopping": "Stopping…",
  "status.error": "Error",

  // Plugins
  "plugins.title": "Install Plugin",
  "plugins.desc": "Install a plugin from one of the following:",
  "plugins.desc_npm": "npm package name, e.g. @rose43/dsh-file or dsh1024@latest",
  "plugins.desc_github": "GitHub repository, e.g. github:owner/repo#v0.1.0",
  "plugins.desc_path": "local path, e.g. /path/to/plugin or ~/my-plugin",
  "plugins.desc_npx": "or paste a full npx command — it runs as-is",
  "plugins.desc_restart": "After installing, restart DSH to activate the plugin.",
  "plugins.placeholder": "plugin name / github:owner/repo#tag / local path / full npx command",
  "plugins.will_run": "Will run",
  "plugins.install": "Install",
  "plugins.installing": "Installing plugin…",
  "plugins.installing_hint":
    "Downloads from GitHub can be slow. Retry messages in the log are normal — please wait and keep the app open.",
  "plugins.retry_hint": "Slow network — the download is retrying automatically and is still in progress. Please keep the app open.",
  "plugins.waiting": "Waiting for command output…",
  "plugins.restart_hint_title": "Plugin installed",
  "plugins.restart_hint_sub": "Restart DeepSeek Harness to make it take effect.",
  "plugins.restart_now": "Restart now",
  "plugins.restarted": "DeepSeek Harness restarted.",
  "plugins.install_failed": "Install failed — see the log above.",

  // Settings
  "settings.language": "Language",
  "settings.language_desc": "Choose the interface language.",
  "settings.language.en": "English",
  "settings.language.zh": "中文",
  "settings.theme": "Theme",
  "settings.theme_desc": "Choose the interface appearance.",
  "settings.theme.system": "Follow system",
  "settings.theme.light": "Light",
  "settings.theme.dark": "Dark",
  "settings.node": "Node.js runtime",
  "settings.node_desc":
    "Optional. Point the launcher at a specific Node.js binary (e.g. /opt/homebrew/bin/node). " +
    "It is used only after it is verified to run; leave empty to rely on automatic detection.",
  "settings.node_placeholder": "e.g. /opt/homebrew/bin/node",
  "settings.node_save": "Save",
  "settings.node_clear": "Clear",
  "settings.diagnose": "Diagnostics",
  "settings.diagnose_desc":
    "Copy a report of exactly what Node.js/npm detection found on this machine — paste it into a bug report.",
  "settings.diagnose_copy": "Copy report",
  "settings.diagnose_copied": "Diagnostics copied to clipboard.",
  "settings.about": "About",
  "settings.about_desc":
    "DSH Launcher installs and starts DeepSeek Harness for you — no command line needed.",
} as const;

export type MessageKey = keyof typeof en;

const zh: Record<MessageKey, string> = {
  // App shell
  "app.loading": "启动中…",
  "app.settings": "设置",
  "app.close_settings": "返回",
  "app.plugins": "插件",
  "app.close_plugins": "返回",

  // Home — one big button
  "home.install": "安装 DeepSeek Harness",
  "home.install_v": "安装 DeepSeek Harness（v{version}）",
  "home.open": "打开 DeepSeek Harness",
  "home.open_v": "打开 DeepSeek Harness（v{version}）",
  "home.busy.installing": "正在安装…",
  "home.busy.opening": "正在打开…",
  "home.busy.updating": "正在更新…",
  "home.busy.starting": "正在启动…",
  "home.busy.stopping": "正在停止…",
  "home.busy.restarting": "正在重启…",
  "home.update": "更新",
  "home.update_v": "更新到 v{version}",
  "home.updated": "DeepSeek Harness 已更新到 v{version}。",
  "home.copy_url": "复制链接",
  "home.start": "启动",
  "home.stop": "停止",
  "home.restart": "重启",

  // Status dots
  "status.stopped": "已停止",
  "status.starting": "启动中…",
  "status.running": "运行中",
  "status.stopping": "停止中…",
  "status.error": "出错了",

  // Plugins
  "plugins.title": "安装插件",
  "plugins.desc": "支持以下插件来源：",
  "plugins.desc_npm": "npm 包名，如 @rose43/dsh-file 或 dsh1024@latest",
  "plugins.desc_github": "GitHub 仓库，如 github:owner/repo#v0.1.0",
  "plugins.desc_path": "本地路径，如 /path/to/plugin 或 ~/my-plugin",
  "plugins.desc_npx": "或直接粘贴以 npx 开头的完整命令（原样执行）",
  "plugins.desc_restart": "安装完成后请重启 DSH 以启用插件。",
  "plugins.placeholder": "插件包名 / github:owner/repo#tag / 本地路径 / 完整 npx 命令",
  "plugins.will_run": "将执行",
  "plugins.install": "安装",
  "plugins.installing": "正在安装插件…",
  "plugins.installing_hint":
    "GitHub 来源下载可能较慢，日志中出现重试提示属正常现象，请耐心等待并保留程序开启。",
  "plugins.retry_hint": "检测到网络重试，下载仍在进行中，请勿关闭程序。",
  "plugins.waiting": "等待命令输出…",
  "plugins.restart_hint_title": "插件已安装",
  "plugins.restart_hint_sub": "重启 DeepSeek Harness 后生效。",
  "plugins.restart_now": "立即重启",
  "plugins.restarted": "DeepSeek Harness 已重启。",
  "plugins.install_failed": "安装失败，请查看上方日志。",

  // Settings
  "settings.language": "语言",
  "settings.language_desc": "选择界面语言。",
  "settings.language.en": "English",
  "settings.language.zh": "中文",
  "settings.theme": "主题",
  "settings.theme_desc": "选择界面外观。",
  "settings.theme.system": "跟随系统",
  "settings.theme.light": "浅色",
  "settings.theme.dark": "深色",
  "settings.node": "Node.js 运行时",
  "settings.node_desc":
    "可选。将启动器指向指定的 Node.js 可执行文件（如 /opt/homebrew/bin/node）。" +
    "仅在验证其可正常运行后才会使用；留空则依赖自动检测。",
  "settings.node_placeholder": "例如 /opt/homebrew/bin/node",
  "settings.node_save": "保存",
  "settings.node_clear": "清除",
  "settings.diagnose": "诊断",
  "settings.diagnose_desc":
    "复制一份报告，记录本机 Node.js/npm 检测的实际结果——可粘贴到问题反馈中。",
  "settings.diagnose_copy": "复制报告",
  "settings.diagnose_copied": "诊断信息已复制到剪贴板。",
  "settings.about": "关于",
  "settings.about_desc":
    "DSH Launcher 帮你自动安装并启动 DeepSeek Harness——无需使用命令行。",
};

export const messages: Record<Language, Record<MessageKey, string>> = {
  en,
  zh,
};
