// Translation dictionaries. The `en` dictionary defines the canonical keys;
// `zh` is type-checked to have exactly the same keys.

export type Language = "en" | "zh";

const en = {
  // App shell
  "app.loading": "Starting…",
  "app.settings": "Settings",
  "app.close_settings": "Back",

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
  "settings.about": "关于",
  "settings.about_desc":
    "DSH Launcher 帮你自动安装并启动 DeepSeek Harness——无需使用命令行。",
};

export const messages: Record<Language, Record<MessageKey, string>> = {
  en,
  zh,
};
