//! Application state shared across commands, tray and startup task.

use crate::browser::{self, BrowserInfo};
use crate::config::{data_dir, dsh_dir, log, Config};
use crate::process::{ProcSnapshot, ProcessManager};
use crate::runtime::{
    self, DshInfo, EnvProgress, EnvStatus, NodeInfo,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;
use tauri::{
    menu::Menu,
    tray::TrayIcon,
    AppHandle, Emitter, Manager,
};

const ENV_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

struct EnvCache {
    node: (Instant, Option<NodeInfo>),
    dsh: (Instant, Option<DshInfo>),
    browsers: (Instant, Vec<BrowserInfo>),
    default: (Instant, Option<browser::BrowserId>),
}

impl EnvCache {
    fn fresh() -> Self {
        // Timestamps start already-expired so the first read of every field
        // performs a real detection instead of returning the placeholder
        // below. A placeholder cached under a *fresh* timestamp is
        // indistinguishable from a real "nothing found": it made the first
        // ENV_CACHE_TTL seconds after launch misreport a compatible Node as
        // "too old", and failed the post-install verification the same way.
        let expired = Instant::now()
            .checked_sub(ENV_CACHE_TTL + std::time::Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        Self {
            node: (expired, None),
            dsh: (expired, None),
            browsers: (expired, Vec::new()),
            default: (expired, None),
        }
    }

    fn invalidate(&mut self) {
        *self = EnvCache::fresh();
    }
}

pub struct AppState {
    pub proc: ProcessManager,
    pub cfg: Mutex<Config>,
    /// True when this launcher instance was started by the OS at login.
    pub launched_by_autostart: bool,
    env_cache: Mutex<EnvCache>,
    /// Tray handle + menu, filled during setup.
    pub tray: Mutex<Option<(TrayIcon, Menu<tauri::Wry>)>>,
}

impl AppState {
    pub fn new(cfg: Config, launched_by_autostart: bool) -> Self {
        Self {
            proc: ProcessManager::new(&cfg.server.host, cfg.server.port),
            cfg: Mutex::new(cfg),
            launched_by_autostart,
            env_cache: Mutex::new(EnvCache::fresh()),
            tray: Mutex::new(None),
        }
    }

    /// Emit the current status to the UI and refresh the tray.
    pub fn push_status(&self, app: &AppHandle) {
        let snap = self.proc.snapshot();
        let _ = app.emit("dsh://status", status_payload(self, &snap));
        if let Some((tray, menu)) = self.tray.lock().unwrap().as_ref() {
            crate::tray::update_tray(self, menu, tray);
        }
    }

    pub fn node(&self) -> Option<NodeInfo> {
        // Read the config first (short lock), then the cache — lock ordering
        // is cfg → env_cache, never nested.
        let configured = { self.cfg.lock().unwrap().node_path.clone() };
        let mut c = self.env_cache.lock().unwrap();
        if c.node.0.elapsed() < ENV_CACHE_TTL {
            return c.node.1.clone();
        }
        let found = match &configured {
            // An explicit, working override wins over auto-detection; an
            // invalid one falls back to it (and logs why it was skipped).
            Some(p) => runtime::detector::detect_node_override(std::path::Path::new(p))
                .or_else(runtime::detector::detect_node),
            None => runtime::detector::detect_node(),
        };
        c.node = (Instant::now(), found.clone());
        found
    }

    pub fn node_any(&self) -> Option<NodeInfo> {
        runtime::detector::detect_node_any()
    }

    /// Directory where DSH is installed / will be installed:
    ///
    /// * the user's configured `dsh_dir` when set, else
    /// * the directory of a detected external install (e.g. an npx cache or
    ///   a global npm install) so updates target the running install, else
    /// * the default managed dir (`~/.dsh-launcher/dsh`).
    ///
    /// Never holds the `cfg` lock while running detection (which may spawn
    /// child processes) — lock ordering is cfg → env_cache, never nested.
    pub fn dsh_target_dir(&self) -> PathBuf {
        let custom = { self.cfg.lock().unwrap().dsh_dir.clone() };
        if let Some(d) = custom {
            let p = PathBuf::from(d);
            // Accept a package-directory value (…/node_modules/@deepseek-ai/dsh)
            // and normalize it to the install root.
            return runtime::detector::normalize_dsh_dir(&p).unwrap_or(p);
        }
        if let Some(found) = runtime::detector::detect_dsh() {
            return found.path;
        }
        dsh_dir()
    }

    pub fn dsh(&self) -> Option<DshInfo> {
        // Read the config first (short lock), then the cache — never hold
        // the env_cache lock while acquiring the cfg lock.
        let custom = { self.cfg.lock().unwrap().dsh_dir.clone() };
        {
            let c = self.env_cache.lock().unwrap();
            if c.dsh.0.elapsed() < ENV_CACHE_TTL {
                return c.dsh.1.clone();
            }
        }
        let found = match &custom {
            // A custom dir was set: only look there (no fallback scan, so
            // updates keep targeting the custom dir).
            Some(dir) => runtime::detector::detect_dsh_in(std::path::Path::new(dir)),
            None => runtime::detector::detect_dsh(),
        };
        self.env_cache.lock().unwrap().dsh = (Instant::now(), found.clone());
        found
    }

    pub fn browsers(&self) -> Vec<BrowserInfo> {
        let mut c = self.env_cache.lock().unwrap();
        if c.browsers.0.elapsed() < ENV_CACHE_TTL {
            return c.browsers.1.clone();
        }
        let found = browser::list_browsers();
        c.browsers = (Instant::now(), found.clone());
        found
    }

    pub fn default_browser(&self) -> Option<browser::BrowserId> {
        let mut c = self.env_cache.lock().unwrap();
        if c.default.0.elapsed() < ENV_CACHE_TTL {
            return c.default.1.clone();
        }
        let found = browser::default_browser();
        c.default = (Instant::now(), found.clone());
        found
    }

    pub fn invalidate_env_cache(&self) {
        self.env_cache.lock().unwrap().invalidate();
    }

    pub fn env_status(&self) -> EnvStatus {
        EnvStatus::from_parts(self.node().as_ref(), self.dsh().as_ref())
    }

    /// Make sure Node.js + DSH are available. Idempotent; streams progress.
    pub async fn ensure_environment(&self, app: &AppHandle) -> Result<(NodeInfo, DshInfo), (String, Option<String>)> {
        let emit = |p: EnvProgress| {
            log(&format!("env[{}]: {}", p.stage, p.message));
            let _ = app.emit("dsh://env", p);
        };

        // --- Node ---
        let node = match self.node() {
            Some(n) => {
                emit(EnvProgress::new(
                    "node",
                    format!(
                        "Node.js v{} detected ({})",
                        n.version,
                        match n.source {
                            runtime::NodeSource::System => "system",
                            runtime::NodeSource::Bundled => "bundled",
                        }
                    ),
                ));
                n
            }
            None => {
                let too_old = self.node_any();
                emit(EnvProgress::new(
                    "node",
                    match &too_old {
                        Some(n) => format!(
                            "Step 1/2: System Node.js v{} at {} is too old (need v{}) — downloading a bundled runtime…",
                            n.version,
                            n.path.display(),
                            runtime::detector::MIN_NODE_MAJOR
                        ),
                        None => "Step 1/2: No compatible Node.js found — downloading a bundled runtime…".into(),
                    },
                ));
                let on_progress: Box<dyn Fn(u64, u64) + Send> = Box::new({
                    let app = app.clone();
                    move |done, total| {
                        let mb = done as f64 / 1024.0 / 1024.0;
                        let msg = if total > 0 {
                            let pct = done as f64 / total as f64 * 100.0;
                            format!("Step 1/2: Downloading Node.js… {mb:.0} MB ({pct:.0}%)")
                        } else {
                            format!("Step 1/2: Downloading Node.js… {mb:.0} MB")
                        };
                        let _ = app.emit("dsh://env", EnvProgress::new("node", msg));
                    }
                });
                let _version = runtime::installer::install_node(Some(on_progress))
                    .await
                    .map_err(|e| (e, None))?;
                self.invalidate_env_cache();
                let node = self
                    .node()
                    .ok_or_else(|| ("Node.js was installed but could not be found afterwards.".into(), None))?;
                emit(EnvProgress::new(
                    "node",
                    format!("Step 1/2: Node.js v{} ready (bundled)", node.version),
                ));
                node
            }
        };

        // --- DSH (always after Node.js is ready) ---
        let dsh = match self.dsh() {
            Some(d) => {
                emit(EnvProgress::new(
                    "dsh",
                    format!("DeepSeek Harness v{} detected", d.version),
                ));
                d
            }
            None => {
                emit(EnvProgress::new(
                    "dsh",
                    "Step 2/2: DeepSeek Harness not found — installing from npm (first run can take a few minutes)…",
                ));
                let on_tail: Box<dyn Fn(String) + Send> = Box::new({
                    let app2 = app.clone();
                    move |tail| {
                        let _ = app2.emit(
                            "dsh://env",
                            EnvProgress::fail("dsh", "npm is reporting errors…", Some(tail)),
                        );
                    }
                });
                let _version = {
                    let target = self.dsh_target_dir();
                    runtime::installer::install_dsh(&node.path, &target, Some(on_tail))
                        .await
                        .map_err(|e| (e, Some("Check the Details for the npm output.".into())))?
                };
                self.invalidate_env_cache();
                let dsh = self
                    .dsh()
                    .ok_or_else(|| ("DSH was installed but could not be found afterwards.".into(), None))?;
                emit(EnvProgress::new(
                    "dsh",
                    format!("Step 2/2: DeepSeek Harness v{} ready", dsh.version),
                ));
                dsh
            }
        };

        Ok((node, dsh))
    }
}

// ---------------------------------------------------------------------------
// Status payload for the UI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BrowserStatus {
    pub selected: Option<String>,
    pub remember: bool,
    pub detected: Vec<BrowserInfo>,
    pub system_default: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LauncherStatus {
    pub process: ProcSnapshot,
    pub env: EnvStatus,
    pub browser: BrowserStatus,
    pub config: Config,
    pub launcher_version: String,
    pub data_dir: String,
}

pub fn status_payload(state: &AppState, proc_snap: &ProcSnapshot) -> LauncherStatus {
    let cfg = state.cfg.lock().unwrap().clone();
    let selected = cfg.browser.r#type.clone();
    let system_default = state.default_browser().map(|b| b.as_str().to_string());
    LauncherStatus {
        process: proc_snap.clone(),
        env: state.env_status(),
        browser: BrowserStatus {
            selected,
            remember: cfg.browser.remember,
            detected: state.browsers(),
            system_default,
        },
        config: cfg,
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir().to_string_lossy().to_string(),
    }
}

// ---------------------------------------------------------------------------
// Startup orchestration
// ---------------------------------------------------------------------------

/// One-time work done after the app starts:
/// 1. adopt an already-running harness — either one we started before
///    (state.json) or **any** service already answering HTTP on the
///    configured port (e.g. a manual `dsh web` in a terminal)
/// 2. if nothing is running, detect the environment (Node + DSH) —
///    **never download anything**
/// 3. start the harness and open the browser — only for manual starts
///    with a remembered browser
///
/// Downloads happen only when the user explicitly clicks an install button
/// (home banner / Settings); startup itself is detection-only.
pub async fn startup(app: AppHandle) {
    let state = app.state::<AppState>();
    let autostart = state.launched_by_autostart;
    let cfg = { state.cfg.lock().unwrap().clone() };

    // 0. Background health watcher: re-checks the harness endpoint every few
    //    seconds so the UI stays in sync when DSH is started or stopped
    //    outside the launcher (e.g. `dsh web` in a terminal, a crash, …).
    spawn_status_watcher(app.clone());

    // 1. Already running? Adopt it (state.json instance, or anything
    //    answering HTTP on the configured port) and show Running. This runs
    //    before the environment check so that a manually started instance is
    //    recognized even when the launcher's own install dir isn't detected.
    if state.proc.try_adopt(&app).await {
        state.push_status(&app);
        return;
    }

    // 1. Environment: detect only. Missing pieces are surfaced in the UI
    //    with explicit "Install" buttons — nothing is downloaded here.
    let node = state.node();
    let dsh = state.dsh();
    if !state.env_status().ready {
        let what = match (node.is_none(), dsh.is_none()) {
            (true, true) => "Node.js and DeepSeek Harness are not installed yet.",
            (true, false) => "No compatible Node.js runtime was found.",
            (false, true) => "DeepSeek Harness is not installed yet.",
            (false, false) => "The environment is not ready.",
        };
        log(&format!("startup: environment not ready — {what}"));
        let _ = app.emit(
            "dsh://env",
            EnvProgress::new(
                "env",
                format!("{what} Click Install to download — nothing is downloaded automatically."),
            ),
        );
        state.push_status(&app);
        return;
    }

    // 2. Start the harness. Environment is already ready (checked above),
    //    so this never downloads anything.
    let (Some(node), Some(dsh)) = (state.node(), state.dsh()) else {
        state.push_status(&app);
        return;
    };
    let outcome = state.proc.start(&app, &node, &dsh).await;
    if outcome.ok {
        // 3. Browser: on a manual start, open the harness in the browser
        //    (remembered one, or the OS default). Never on autostart —
        //    the user can open it from the home button instead.
        if !autostart && cfg.open_browser_on_start {
            crate::commands::open_stored_browser(&state, &app);
        }
    }
    state.push_status(&app);
}

/// Background loop that keeps the UI's process status honest. Runs forever;
/// the app only exits via `quit_app`, so this is fine.
fn spawn_status_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let state = app.state::<AppState>();
            state.proc.poll_status(&app).await;
        }
    });
}

pub fn notify_ready(app: &AppHandle) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("DeepSeek Harness is ready")
        .body("You can keep using it from the system tray — DSH stays running.")
        .show();
}

pub fn notify_error(app: &AppHandle, message: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app
        .notification()
        .builder()
        .title("Unable to start DeepSeek Harness")
        .body(message)
        .show();
}
