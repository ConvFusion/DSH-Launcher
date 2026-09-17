//! Tauri commands — the IPC surface for the frontend.

use crate::browser::{self, BrowserId};
use crate::config::{self, log, Config};
use crate::process::{health, StartOutcome};
use crate::runtime::{self, EnvProgress};
use crate::state::{notify_error, status_payload, AppState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> crate::state::LauncherStatus {
    status_payload(&state, &state.proc.snapshot())
}

/// Home "refresh" button: force a **full re-detection** of the environment
/// (Node.js, DSH, browsers) by expiring the env cache, then return a fresh
/// status payload. This makes a manual refresh behave like reopening the
/// app, instead of returning values that are still within the 30s env-cache
/// TTL (which `get_status` alone would do).
#[tauri::command]
pub fn refresh_status(state: State<'_, AppState>) -> crate::state::LauncherStatus {
    state.invalidate_env_cache();
    status_payload(&state, &state.proc.snapshot())
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvReport {
    pub ready: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub error_details: Option<String>,
}

#[tauri::command]
pub async fn ensure_environment(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<EnvReport, String> {
    let report = match state.ensure_environment(&app).await {
        Ok(_) => EnvReport {
            ready: true,
            message: None,
            error: None,
            error_details: None,
        },
        Err((msg, details)) => EnvReport {
            ready: false,
            message: Some(msg.clone()),
            error: Some(msg),
            error_details: details,
        },
    };
    let snap = state.proc.snapshot();
    let _ = app.emit("dsh://status", status_payload(&state, &snap));
    Ok(report)
}

#[tauri::command]
pub async fn install_node_runtime(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let on_progress: Box<dyn Fn(u64, u64) + Send> = Box::new({
        let app = app.clone();
        move |done, total| {
            let mb = done as f64 / 1024.0 / 1024.0;
            let msg = if total > 0 {
                format!(
                    "Downloading Node.js… {mb:.0} MB ({:.0}%)",
                    done as f64 / total as f64 * 100.0
                )
            } else {
                format!("Downloading Node.js… {mb:.0} MB")
            };
            let _ = app.emit("dsh://env", EnvProgress::new("node", msg));
        }
    });
    let version = runtime::installer::install_node(Some(on_progress)).await?;
    state.invalidate_env_cache();
    let snap = state.proc.snapshot();
    let _ = app.emit("dsh://status", status_payload(&state, &snap));
    Ok(version)
}

#[tauri::command]
pub async fn install_dsh_package(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    let node = state
        .node()
        .ok_or_else(|| "No compatible Node.js runtime is available yet.".to_string())?;
    // Stream the command and every npm output line to the UI (`dsh://update`)
    // so the home page can show a live console log while the button reads
    // "Updating…" — a cold npm install can take minutes of otherwise silent
    // waiting.
    let on_line: runtime::installer::LineSink = {
        let app = app.clone();
        std::sync::Arc::new(move |line: String| {
            let _ = app.emit("dsh://update", line);
        })
    };
    let target = state.dsh_target_dir();
    let version =
        runtime::installer::install_dsh(&node.path, &target, Some(on_line), None).await?;
    state.invalidate_env_cache();
    let snap = state.proc.snapshot();
    let _ = app.emit("dsh://status", status_payload(&state, &snap));
    Ok(version)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub update_available: bool,
}

#[tauri::command]
pub async fn check_dsh_update(state: State<'_, AppState>) -> Result<UpdateInfo, String> {
    let installed = state.dsh().map(|d| d.version);
    match runtime::installer::latest_dsh_version().await {
        Ok(latest) => {
            // Semver-aware comparison (handles pre-releases like 0.1.1-rc.2).
            let update_available = match (
                installed.as_deref().and_then(|v| semver::Version::parse(v).ok()),
                semver::Version::parse(&latest).ok(),
            ) {
                (Some(inst), Some(lat)) => inst < lat,
                _ => installed.as_deref().map(|i| i != latest).unwrap_or(true),
            };
            Ok(UpdateInfo {
                installed,
                latest: Some(latest),
                update_available,
            })
        }
        Err(e) => {
            log(&format!("update check failed: {e}"));
            Ok(UpdateInfo {
                installed,
                latest: None,
                update_available: false,
            })
        }
    }
}

/// Check for a newer version of the **launcher itself** by querying GitHub
/// Releases. Returns the installed version, the latest release version, and
/// whether an update is available (semver-aware comparison).
#[tauri::command]
pub async fn check_launcher_update() -> Result<UpdateInfo, String> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let url = "https://api.github.com/repos/ConvFusion/DSH-Launcher/releases/latest";

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .user_agent("dsh-launcher/update-check")
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("update check request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "update check returned {} : {}",
            status,
            body
        ));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse update check response: {e}"))?;

    let tag = json["tag_name"]
        .as_str()
        .ok_or("release tag_name not found")?;

    // Strip the leading "v" if present.
    let latest = tag.strip_prefix('v').unwrap_or(tag).to_string();

    // Semver-aware comparison.
    let update_available = match (
        semver::Version::parse(&installed).ok(),
        semver::Version::parse(&latest).ok(),
    ) {
        (Some(inst), Some(lat)) => inst < lat,
        _ => installed != latest,
    };

    Ok(UpdateInfo {
        installed: Some(installed),
        latest: Some(latest),
        update_available,
    })
}

/// Open the launcher's GitHub Releases page in the user's default browser.
///
/// The home page's "update available" banner routes through here on purpose:
/// a plain `<a target="_blank">` is a **no-op** inside the Tauri webview (no
/// `on_new_window` handler is registered, so the webview refuses to create the
/// window and the click is silently dropped), and a plain `<a href>` would
/// navigate the launcher webview itself away from the app.
///
/// The URL is fixed here rather than passed in from the frontend, so the IPC
/// surface cannot be used to open arbitrary URLs.
#[tauri::command]
pub fn open_releases_page() -> Result<(), String> {
    const RELEASES_URL: &str = "https://github.com/ConvFusion/DSH-Launcher/releases";
    browser::open_url_default(RELEASES_URL)
}

// ---------------------------------------------------------------------------
// Process control
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn start_dsh(
    state: State<'_, AppState>,
    app: AppHandle,
    open_browser: Option<bool>,
) -> Result<StartOutcome, String> {
    Ok(start_dsh_inner(&state, &app, open_browser).await)
}

async fn start_dsh_inner(
    state: &AppState,
    app: &AppHandle,
    open_browser: Option<bool>,
) -> StartOutcome {
    let cfg = state.cfg.lock().unwrap().clone();
    let open_browser = open_browser.unwrap_or(cfg.open_browser_on_start);

    // Already running (started before, or adopted externally from the
    // configured port)? Nothing to start — just report it.
    if state.proc.try_adopt(app).await {
        let port = state.proc.snapshot().port;
        if open_browser {
            open_stored_browser(state, app);
        }
        return StartOutcome::already_running(port);
    }

    // Detection only — starting the service never downloads anything.
    // If Node.js or DSH is missing, tell the user to install it explicitly
    // (home banner / Settings) instead of silently starting a download.
    let (node, dsh) = match (state.node(), state.dsh()) {
        (Some(n), Some(d)) => (n, d),
        _ => {
            let missing = match (state.node().is_none(), state.dsh().is_none()) {
                (true, true) => "Node.js and DeepSeek Harness are not installed yet",
                (true, false) => "No compatible Node.js runtime was found",
                (false, true) => "DeepSeek Harness is not installed yet",
                (false, false) => "The environment is not ready",
            };
            let msg = format!(
                "{missing}. Click Install to download it, then start again — \
                 DSH Launcher never downloads anything without your click."
            );
            let _ = app.emit("dsh://env", EnvProgress::fail("env", msg.clone(), None));
            return StartOutcome::error(msg, None);
        }
    };
    let outcome = state.proc.start(app, &node, &dsh).await;
    if outcome.ok && open_browser {
        // The harness prints its `?token=…` banner just after it starts
        // answering HTTP, so the snapshot taken above can still hold the bare
        // URL — wait briefly so the browser opens an authorized URL.
        let _ = state.proc.wait_for_token_url().await;
        open_stored_browser(state, app);
    }
    if !outcome.ok && outcome.kind == "error" {
        notify_error(app, outcome.message.as_deref().unwrap_or("See Details."));
    }
    outcome
}

pub async fn stop_dsh_impl(state: &AppState, app: &AppHandle) -> Result<(), String> {
    state.proc.stop(app).await
}

#[tauri::command]
pub async fn stop_dsh(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    stop_dsh_impl(&state, &app).await
}

pub async fn restart_dsh_impl(
    state: &AppState,
    app: &AppHandle,
    open_browser: Option<bool>,
) -> StartOutcome {
    let _ = state.proc.stop(app).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    start_dsh_inner(state, app, open_browser).await
}

#[tauri::command]
pub async fn restart_dsh(
    state: State<'_, AppState>,
    app: AppHandle,
    open_browser: Option<bool>,
) -> Result<StartOutcome, String> {
    Ok(restart_dsh_impl(&state, &app, open_browser).await)
}

/// Open the harness URL in the browser (tray "Open Harness" / home button).
/// Uses the remembered browser if any, otherwise the OS default — novices
/// should never be asked to pick a browser.
pub(crate) fn open_stored_browser(state: &AppState, app: &AppHandle) {
    let cfg = state.cfg.lock().unwrap().clone();
    match cfg.browser.r#type.as_deref().and_then(BrowserId::from_str) {
        Some(id) => {
            let url = state.proc.snapshot().url.clone();
            match browser::open_url(id, &url) {
                Ok(()) => crate::state::notify_ready(app),
                Err(e) => log(&format!("open browser failed: {e}")),
            }
        }
        None => {
            log("open_harness: no configured browser, using the OS default");
            let url = state.proc.snapshot().url.clone();
            match browser::open_url_default(&url) {
                Ok(()) => crate::state::notify_ready(app),
                Err(e) => log(&format!("open default browser failed: {e}")),
            }
        }
    }
}

/// Open the harness URL in the stored browser (tray "Open Harness").
/// If the service is not running, starts it first.
pub async fn open_harness_impl(state: &AppState, app: &AppHandle) -> Result<(), String> {
    let snap = state.proc.snapshot();
    if snap.state == crate::process::ProcessState::Running {
        // Already up: the token may have been parsed after the last status
        // push, in which case the snapshot's URL is still the bare one.
        let _ = state.proc.wait_for_token_url().await;
        open_stored_browser(state, app);
        return Ok(());
    }
    // Not running: start it, then open.
    let outcome = start_dsh_inner(state, app, Some(true)).await;
    if outcome.ok {
        Ok(())
    } else {
        Err(outcome.message.unwrap_or_else(|| "Unable to start DeepSeek Harness.".into()))
    }
}

#[tauri::command]
pub async fn open_harness(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    open_harness_impl(&state, &app).await
}

// ---------------------------------------------------------------------------
// Browser
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn detect_browsers(state: State<'_, AppState>) -> Vec<browser::BrowserInfo> {
    state.invalidate_env_cache();
    state.browsers()
}

/// Append a raw line from the frontend to logs/debug.log — used to capture
/// exactly what data the UI receives (IPC round-trip diagnostics).
#[tauri::command]
pub fn write_debug(text: String) {
    use std::io::Write;
    let path = config::logs_dir().join("debug.log");
    let ts = config::now_stamp();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{ts} {text}");
    }
}

#[tauri::command]
pub fn select_browser(
    state: State<'_, AppState>,
    app: AppHandle,
    browser_id: String,
    remember: bool,
) -> Result<(), String> {
    let id = BrowserId::from_str(&browser_id)
        .ok_or_else(|| format!("unknown browser: {browser_id}"))?;
    let installed = state
        .browsers()
        .into_iter()
        .find(|b| b.id == id.as_str())
        .map(|b| b.installed)
        .unwrap_or(false);
    if !installed {
        return Err(format!("{} is not installed on this computer.", id.display_name()));
    }
    let cfg = {
        let mut g = state.cfg.lock().unwrap();
        g.browser.r#type = Some(id.as_str().to_string());
        g.browser.remember = remember;
        g.clone()
    };
    cfg.save()?;
    state.invalidate_env_cache();
    let snap = state.proc.snapshot();
    let _ = app.emit("dsh://status", status_payload(&state, &snap));
    Ok(())
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.cfg.lock().unwrap().clone()
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ConfigPatch {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub open_browser_on_start: Option<bool>,
    pub language: Option<String>,
    pub theme: Option<String>,
    pub dsh_dir: Option<String>,
    pub node_path: Option<String>,
}

#[tauri::command]
pub fn update_config(
    state: State<'_, AppState>,
    app: AppHandle,
    patch: ConfigPatch,
) -> Result<Config, String> {
    let mut cfg = state.cfg.lock().unwrap().clone();
    if let Some(host) = patch.host.filter(|h| !h.trim().is_empty()) {
        let host = host.trim().to_string();
        if !host.contains('.') && !host.eq_ignore_ascii_case("localhost") && host.parse::<std::net::IpAddr>().is_err() {
            return Err("Host must be an IP address (e.g. 127.0.0.1) or localhost.".into());
        }
        cfg.server.host = host;
    }
    if let Some(port) = patch.port {
        if !(1024..=65535).contains(&port) {
            return Err("Port must be between 1024 and 65535.".into());
        }
        cfg.server.port = port;
    }
    if let Some(v) = patch.open_browser_on_start {
        cfg.open_browser_on_start = v;
    }
    if let Some(lang) = patch.language {
        if lang != "en" && lang != "zh" {
            return Err("Language must be \"en\" or \"zh\".".into());
        }
        cfg.language = lang;
    }
    if let Some(theme) = patch.theme {
        if theme != "system" && theme != "light" && theme != "dark" {
            return Err("Theme must be \"system\", \"light\" or \"dark\".".into());
        }
        cfg.theme = theme;
    }
    if let Some(dir) = patch.dsh_dir {
        let dir = dir.trim().to_string();
        if dir.is_empty() {
            cfg.dsh_dir = None; // clear → back to default managed dir
        } else {
            // Accept a relative path expanded against home, and validate it.
            let expanded = if dir.starts_with("~/") {
                dirs::home_dir()
                    .map(|h| h.join(dir.trim_start_matches("~/")))
                    .unwrap_or_else(|| std::path::PathBuf::from(&dir))
            } else {
                std::path::PathBuf::from(&dir)
            };
            if !expanded.is_dir() {
                return Err(format!(
                    "Directory does not exist: {} — create it first or use the default.",
                    expanded.display()
                ));
            }
            // If the user pointed at the package directory itself
            // (…/node_modules/@deepseek-ai/dsh — what a file picker yields),
            // store the install root instead so detection and updates work.
            let canonical = crate::runtime::detector::normalize_dsh_dir(&expanded)
                .unwrap_or(expanded.clone());
            cfg.dsh_dir = Some(canonical.to_string_lossy().to_string());
        }
    }
    if let Some(p) = patch.node_path {
        let p = p.trim().to_string();
        if p.is_empty() {
            cfg.node_path = None; // clear → back to auto-detection
        } else {
            // Accept `~/…` expanded against home, then prove the binary
            // works (executes + able to run DSH; see `node_compatible`)
            // before trusting it.
            let expanded = if p.starts_with("~/") {
                dirs::home_dir()
                    .map(|h| h.join(p.trim_start_matches("~/")))
                    .unwrap_or_else(|| std::path::PathBuf::from(&p))
            } else {
                std::path::PathBuf::from(&p)
            };
            if !expanded.is_file() {
                return Err(format!(
                    "Node.js binary not found at {} — check the path.",
                    expanded.display()
                ));
            }
            match crate::runtime::detector::detect_node_override(&expanded) {
                Some(n) => {
                    log(&format!(
                        "node_path override set to {} (v{})",
                        expanded.display(),
                        n.version
                    ));
                    cfg.node_path = Some(expanded.to_string_lossy().to_string());
                }
                None => {
                    return Err(format!(
                        "{} does not provide a usable Node.js (DeepSeek Harness needs {}). \
                         Auto-detection will be used.",
                        expanded.display(),
                        crate::runtime::detector::MIN_NODE_LABEL
                    ));
                }
            }
        }
    }
    cfg.save()?;
    // Apply to the live manager (affects the next start).
    state.proc.set_target(&cfg.server.host, cfg.server.port);
    state.invalidate_env_cache();
    let snap = state.proc.snapshot();
    let _ = app.emit("dsh://status", status_payload(&state, &snap));
    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Autostart
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn set_autostart(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| format!("Could not enable autostart: {e}"))?;
    } else {
        mgr.disable().map_err(|e| format!("Could not disable autostart: {e}"))?;
    }
    let cfg = {
        let mut g = state.cfg.lock().unwrap();
        g.autostart = enabled;
        g.clone()
    };
    cfg.save()?;
    Ok(enabled)
}

// ---------------------------------------------------------------------------
// Logs & misc
// ---------------------------------------------------------------------------

/// Full report of what the launcher detects on **this** machine (PATH,
/// login-shell resolution, every Node.js candidate, the selected Node, and
/// the npm/npx CLI it resolves). Each line is also written to launcher.log.
/// Intended for diagnosing environment issues on machines we can't inspect
/// directly — e.g. "paste this into the bug report."
#[tauri::command]
pub fn diagnose_environment() -> Vec<String> {
    let report = crate::runtime::detector::env_diagnostics();
    log(&format!(
        "[diagnose] {} lines",
        report.len()
    ));
    for line in &report {
        log(&format!("[diagnose] {line}"));
    }
    report
}

/// Bundle everything needed to diagnose a problem on **this** machine into a
/// single zip file the user can send for support:
///
/// ```text
/// logs/launcher.log        startup + lifecycle events
/// logs/install.log(.old)   full Node/DSH/plugin install command output
/// logs/harness.log(.old)   DSH runtime (web server) output
/// config.json, state.json  user settings + last process record
/// diagnostics.txt          live environment report (node/npm/dsh detection)
/// meta.txt                 launcher version, OS, arch, data dir, timestamp
/// ```
///
/// The zip is written into the app data directory (`~/.dsh-launcher`) and
/// revealed (selected) in the OS file manager, so even non-technical users
/// can find and attach it. Returns the path of the created file.
#[tauri::command]
pub fn collect_logs() -> Result<String, String> {
    let dir = config::data_dir();
    let stamp = config::now_stamp().replace(':', "-");
    let dest = dir.join(format!("dsh-launcher-logs-{stamp}.zip"));

    let mut zw = zip::ZipWriter::new(
        std::fs::File::create(&dest).map_err(|e| format!("create {}: {e}", dest.display()))?,
    );
    let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let logs = config::logs_dir();
    for name in [
        "launcher.log",
        "install.log",
        "install.log.old",
        "harness.log",
        "harness.log.old",
    ] {
        zip_add_file(&mut zw, &opts, &format!("logs/{name}"), &logs.join(name))?;
    }
    zip_add_file(&mut zw, &opts, "config.json", &config::config_path())?;
    zip_add_file(&mut zw, &opts, "state.json", &config::state_path())?;

    // Live environment report — run for real, so it reflects the machine.
    let diag: String =
        crate::runtime::detector::env_diagnostics().join("\n") + "\n";
    zip_write_text(&mut zw, &opts, "diagnostics.txt", &diag)?;

    let meta = format!(
        "dsh-launcher version: {}\ngenerated at: {}\nos: {}\narch: {}\ndata dir: {}\n",
        env!("CARGO_PKG_VERSION"),
        config::now_stamp(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        dir.display(),
    );
    zip_write_text(&mut zw, &opts, "meta.txt", &meta)?;

    zw.finish().map_err(|e| format!("finish zip: {e}"))?;
    let path = dest.to_string_lossy().to_string();
    log(&format!("log bundle written: {path}"));
    reveal_path(&dest);
    Ok(path)
}

fn zip_add_file(
    zw: &mut zip::ZipWriter<std::fs::File>,
    opts: &zip::write::FileOptions<'_, ()>,
    name: &str,
    path: &std::path::Path,
) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let data = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    zip_write_bytes(zw, opts, name, &data)
}

fn zip_write_bytes(
    zw: &mut zip::ZipWriter<std::fs::File>,
    opts: &zip::write::FileOptions<'_, ()>,
    name: &str,
    data: &[u8],
) -> Result<(), String> {
    use std::io::Write;
    zw.start_file(name, opts.clone())
        .map_err(|e| format!("zip {name}: {e}"))?;
    zw.write_all(data).map_err(|e| format!("zip {name}: {e}"))
}

fn zip_write_text(
    zw: &mut zip::ZipWriter<std::fs::File>,
    opts: &zip::write::FileOptions<'_, ()>,
    name: &str,
    text: &str,
) -> Result<(), String> {
    zip_write_bytes(zw, opts, name, text.as_bytes())
}

/// Reveal a file in the OS file manager, selected when the OS allows it.
fn reveal_path(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer.exe")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(parent) = path.parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

#[tauri::command]
pub fn read_log(name: String, lines: Option<u32>) -> Result<String, String> {
    config::read_log_tail(&name, lines.unwrap_or(200).min(2000) as usize)
}

#[tauri::command]
pub fn open_log_dir() -> Result<(), String> {
    let dir = config::logs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    open_directory(&dir)
}

#[cfg(target_os = "macos")]
fn open_directory(dir: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
fn open_directory(dir: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn open_directory(_dir: &std::path::Path) -> Result<(), String> {
    Err("Unsupported platform".into())
}

/// Quit the launcher: stop the harness first, then exit.
#[tauri::command]
pub async fn quit_app(state: State<'_, AppState>, app: AppHandle) -> Result<(), String> {
    let _ = state.proc.stop(&app).await;
    app.exit(0);
    Ok(())
}

// ---------------------------------------------------------------------------
// DSH plugins
// ---------------------------------------------------------------------------

/// Characters that would be meaningful to a shell. Plugin commands are
/// executed directly (never through a shell), so any of these in the input
/// is rejected up front.
fn plugin_input_forbidden(c: char) -> bool {
    matches!(c, ';' | '|' | '&' | '<' | '>' | '$' | '`' | '\n' | '\r' | '"' | '\'')
}

/// Validate a bare plugin source and normalize it. Accepted forms:
///
/// * npm package name: `@scope/name` or `name`, optionally `@version`
///   (e.g. `@rose43/dsh-file`, `dsh1024@latest`)
/// * GitHub reference: `github:owner/repo` or `github:owner/repo#tag/branch`
/// * local path: `/abs/path`, `~/path`, `./rel`, `C:\…`
///
/// A leading `~/` is expanded here because no shell will do it for us.
fn normalize_plugin_source(input: &str) -> Result<String, String> {
    if input.len() > 512 {
        return Err("The plugin source is too long.".into());
    }
    if input.chars().any(plugin_input_forbidden) {
        return Err(
            "The plugin source contains characters that are not allowed (e.g. ; | & $ ` < > ' \")."
                .into(),
        );
    }

    // GitHub reference: github:owner/repo[#ref]
    if let Some(rest) = input.strip_prefix("github:") {
        let (repo, ref_part) = rest.split_once('#').unwrap_or((rest, ""));
        let repo_ok = repo.split('/').count() == 2
            && repo
                .split('/')
                .all(|seg| {
                    !seg.is_empty()
                        && seg
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                });
        let ref_ok = ref_part.is_empty()
            || ref_part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
        if repo_ok && ref_ok {
            return Ok(input.to_string());
        }
        return Err(
            "Invalid GitHub reference — use `github:owner/repo` or `github:owner/repo#tag`."
                .into(),
        );
    }

    // Local path: absolute, home-relative, or a Windows drive letter.
    let b = input.as_bytes();
    let is_path = input.starts_with('/')
        || input.starts_with("~/")
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with(".\\")
        || input.starts_with("..\\")
        || (b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'));
    if is_path {
        if let Some(rest) = input.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                return Ok(home.join(rest).to_string_lossy().to_string());
            }
        }
        return Ok(input.to_string());
    }

    // npm package name, optionally suffixed with a version.
    if !input.is_empty() && !input.starts_with('-') {
        let body = input.strip_prefix('@').unwrap_or(input);
        let seg = body.rsplit_once('@').map(|(l, _)| l).unwrap_or(body);
        let shape_ok = if input.starts_with('@') {
            // Scoped: exactly scope/name.
            seg.split('/').count() == 2 && !seg[1..].starts_with('/') && !seg[1..].is_empty()
        } else {
            !seg.contains('/')
        };
        let name_ok = shape_ok
            && !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
        let all_ok = body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@'));
        if name_ok && all_ok {
            return Ok(input.to_string());
        }
    }

    Err(
        "Invalid plugin source. Use an npm package name (e.g. @rose43/dsh-file), a GitHub reference (github:owner/repo[#tag]) or a local path."
            .into(),
    )
}

/// Build the argument vector for npx from the user's input.
///
/// * Input starting with `npx …` → a complete command, used **as-is**
///   (it must still target `@deepseek-ai/dsh` and stay shell-safe).
/// * Anything else → a bare plugin source, wrapped into the standard:
///   `npx -y --package @deepseek-ai/dsh dsh plugin --profile web add <source>`
fn plugin_npx_args(input: &str) -> Result<Vec<String>, String> {
    // "npx" followed by a space (or alone) → a full command, not an
    // npm package name that happens to start with "npx".
    if input == "npx" || input.starts_with("npx ") {
        if input.chars().any(plugin_input_forbidden) {
            return Err(
                "The command contains characters that are not allowed (e.g. ; | & $ ` < > ' \")."
                    .into(),
            );
        }
        let mut tokens: Vec<String> = input.split_whitespace().map(str::to_string).collect();
        if tokens.len() < 2 {
            return Err("The npx command is missing arguments.".into());
        }
        if !input.contains("@deepseek-ai/dsh") {
            return Err(
                "The command must target the @deepseek-ai/dsh package, e.g. `npx -y --package @deepseek-ai/dsh dsh plugin --profile web add …`."
                    .into(),
            );
        }
        // `dsh plugin` requires `--profile <name>`. Commands written before
        // that was known — including the one-click links an earlier build
        // offered — omit it and die with "required option '--profile <name>'
        // not specified", so supply the profile this launcher boots instead of
        // failing. An explicit `--profile` (any value) is always left alone.
        if let Some(i) = tokens.iter().position(|t| t == "plugin") {
            if !tokens[i..].iter().any(|t| t == "--profile") {
                tokens.insert(i + 1, "--profile".to_string());
                tokens.insert(i + 2, "web".to_string());
            }
        }
        return Ok(tokens[1..].to_vec());
    }
    let source = normalize_plugin_source(input)?;
    Ok(vec![
        "-y".to_string(),
        "--package".to_string(),
        "@deepseek-ai/dsh".to_string(),
        "dsh".to_string(),
        "plugin".to_string(),
        "--profile".to_string(),
        "web".to_string(),
        "add".to_string(),
        source,
    ])
}

/// Install a DSH plugin. Runs the (validated) command through the managed
/// Node runtime's npx — `node npx-cli.js <args>` — so it works identically
/// on macOS and Windows without a shell, and streams every output line to
/// the UI (`dsh://plugin`). See [`plugin_npx_args`] for the two supported
/// input forms.
///
/// A failed attempt (non-zero exit, e.g. a flaky GitHub download) is
/// retried once; the retry is announced in the log stream so the UI can
/// tell the user the work is still in progress.
#[tauri::command]
pub async fn install_dsh_plugin(
    state: State<'_, AppState>,
    app: AppHandle,
    name: String,
) -> Result<String, String> {
    let input = name.trim().to_string();
    if input.is_empty() {
        return Err("Please enter a plugin name or a full npx command.".into());
    }
    let npx_args = plugin_npx_args(&input)?;

    // Idempotency guard for the launcher's own plugins: if the package is
    // already in the web profile's dependencies, skip the install instead of
    // pnpm-adding it a second time — a duplicate add can leave two loader
    // entries with the same id (e.g. "additive") and brick harness boot with
    // "duplicate loader entry id: …".
    if let Some(pkg) = supported_plugin_name(&input) {
        let deps = read_profile_deps();
        if deps.iter().any(|d| d == &pkg) {
            let msg = format!(
                "[launcher] {pkg} is already installed in the web profile — skipping. \
                 If DeepSeek Harness crashes at boot, remove the plugin and reinstall after \
                 the plugin author publishes a compatible build."
            );
            log(&msg);
            config::install_log_line(&msg);
            let _ = app.emit("dsh://plugin", msg);
            return Ok("already-installed".into());
        }
    }

    let node = state
        .node()
        .ok_or("No compatible Node.js runtime is available — install DeepSeek Harness first.")?;
    let npx_cli = crate::runtime::detector::npx_cli_for(&node.path).ok_or_else(|| {
        format!(
            "Cannot locate npx for the Node runtime at {}.",
            node.path.display()
        )
    })?;

    log(&format!(
        "plugin command: {} {}",
        npx_cli.display(),
        npx_args.join(" ")
    ));
    config::install_log_line(&format!(
        "$ node {} {}",
        npx_cli.display(),
        npx_args.join(" ")
    ));

    // `dsh plugin` shells out to pnpm, so make sure one exists *before*
    // running the command — otherwise it exits 127 with "pnpm not found on
    // PATH", which a retry can never fix.
    let npm_cli = {
        let sibling = npx_cli.with_file_name("npm-cli.js");
        if sibling.exists() {
            sibling
        } else {
            crate::runtime::detector::npm_cli_for(&node.path).unwrap_or(sibling)
        }
    };
    let pnpm_dir = match ensure_pnpm(&node, &npm_cli, &app).await {
        Ok(dir) => dir,
        Err(e) => {
            config::install_log_line(&format!("pnpm setup FAILED: {e}"));
            return Err(e);
        }
    };
    if let Some(dir) = pnpm_dir.as_ref() {
        log(&format!("plugin command: pnpm from {}", dir.display()));
    }

    const MAX_ATTEMPTS: u32 = 2;
    let mut last_exit: Option<i32> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            log(&format!(
                "plugin command: retrying (attempt {attempt}/{MAX_ATTEMPTS})"
            ));
            let _ = app.emit(
                "dsh://plugin",
                format!(
                    "[launcher] previous attempt failed (exit {}), retrying ({attempt}/{MAX_ATTEMPTS})…",
                    last_exit.unwrap_or(-1)
                ),
            );
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }

        match run_plugin_once(&node, &npx_cli, &npx_args, &app, &input, pnpm_dir.as_deref()).await {
            PluginRun::Success => return Ok(input),
            PluginRun::FailedExit(code) => {
                last_exit = code;
            }
            PluginRun::Fatal(e) => return Err(e),
        }
    }

    config::install_log_line(&format!(
        "plugin install FAILED after {MAX_ATTEMPTS} attempts (exit code {:?})",
        last_exit
    ));
    Err(format!(
        "The plugin install failed after {MAX_ATTEMPTS} attempts (exit code {:?}) — see the log above.",
        last_exit
    ))
}

/// pnpm major the launcher installs for itself when the machine has none.
///
/// `dsh plugin` is a *thin pnpm forwarder*: it resolves a profile directory and
/// runs `spawnSync("pnpm", …)` with the inherited PATH, so plugin management
/// needs a real pnpm from somewhere. Node ≤ 24 shipped corepack (whose shim
/// provides one), but Node 25 dropped corepack — with a bundled runtime there
/// is simply no pnpm, and `dsh plugin` exits 127 with
/// "pnpm not found on PATH".
const PNPM_VERSION: &str = "11";

/// The launcher's own pnpm, when it has already been installed.
///
/// `npm install -g --prefix X` puts the shim in `X/bin` on unix; on Windows
/// the global bin directory *is* the prefix (`X\pnpm.cmd` beside
/// `X\node_modules`), so both layouts are checked.
fn provisioned_pnpm_dir() -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let tools = config::tools_dir();
    [tools.join("bin"), tools]
        .into_iter()
        .find(|dir| dir.join(exe).exists())
}

/// Is a *working* `pnpm` already reachable through PATH?
///
/// Corepack shims answer this question honestly: an uncached (or cleaned)
/// version makes the shim exit non-zero, and such a shim must not be trusted —
/// the launcher installs its own copy instead.
fn pnpm_on_path() -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "pnpm --version"]);
        c
    } else {
        let mut c = std::process::Command::new("pnpm");
        c.arg("--version");
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Make sure the plugin command can find a pnpm.
///
/// Returns the directory to prepend to `PATH`, or `None` when an existing
/// pnpm on PATH can be used as-is.
async fn ensure_pnpm(
    node: &crate::runtime::NodeInfo,
    npm_cli: &std::path::Path,
    app: &AppHandle,
) -> Result<Option<std::path::PathBuf>, String> {
    if let Some(dir) = provisioned_pnpm_dir() {
        return Ok(Some(dir));
    }
    if pnpm_on_path() {
        log("pnpm: using the copy already on PATH");
        return Ok(None);
    }

    let tools = config::tools_dir();
    std::fs::create_dir_all(&tools)
        .map_err(|e| format!("Cannot create {}: {e}", tools.display()))?;
    let args: Vec<String> = vec![
        "install".into(),
        "-g".into(),
        format!("pnpm@{PNPM_VERSION}"),
        "--prefix".into(),
        tools.display().to_string(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--loglevel".into(),
        "error".into(),
    ];
    log(&format!(
        "pnpm not found — installing pnpm@{PNPM_VERSION} into {}",
        tools.display()
    ));
    config::install_log_line(&format!(
        "$ node {} {}",
        npm_cli.display(),
        args.join(" ")
    ));
    let _ = app.emit(
        "dsh://plugin",
        format!(
            "[launcher] `dsh plugin` forwards to pnpm and none was found on this machine — \
             installing pnpm@{PNPM_VERSION} into the launcher's own directory (one time only)…"
        ),
    );

    match run_plugin_once(node, npm_cli, &args, app, "setup: pnpm", None).await {
        PluginRun::Success => provisioned_pnpm_dir().map(Some).ok_or_else(|| {
            format!(
                "pnpm was installed but no pnpm executable appeared in {}.",
                tools.display()
            )
        }),
        PluginRun::FailedExit(code) => Err(format!(
            "Could not install pnpm (exit code {:?}) — plugin management needs pnpm. \
             Install it yourself (`npm install -g pnpm`) and try again.",
            code
        )),
        PluginRun::Fatal(e) => Err(e),
    }
}

/// Outcome of one plugin command run.
enum PluginRun {
    Success,
    /// Process exited non-zero — worth a retry (often a flaky download).
    FailedExit(Option<i32>),
    /// Spawn/wait/timeout failure — retrying won't help.
    Fatal(String),
}

/// Run the plugin command once, streaming stdout/stderr to the UI, and
/// classify the outcome.
async fn run_plugin_once(
    node: &crate::runtime::NodeInfo,
    npx_cli: &std::path::Path,
    npx_args: &[String],
    app: &AppHandle,
    input: &str,
    pnpm_dir: Option<&std::path::Path>,
) -> PluginRun {
    let mut cmd = tokio::process::Command::new(&node.path);
    cmd.arg(npx_cli).args(npx_args);
    let node_dir = node
        .path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let old_path = std::env::var("PATH").unwrap_or_default();
    // The launcher's own pnpm comes first (when it has one), then the node
    // directory: `dsh plugin` runs a bare `pnpm`, so it must be on PATH.
    let prefix = match pnpm_dir {
        Some(dir) => format!("{}{sep}{}{sep}", dir.display(), node_dir.display()),
        None => format!("{}{sep}", node_dir.display()),
    };
    cmd.env("PATH", format!("{prefix}{old_path}"));
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            config::install_log_line(&format!(
                "plugin command FAILED to start: {e}"
            ));
            return PluginRun::Fatal(format!("Could not start the plugin command: {e}"));
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let input_log = input.to_string();
    // Two owned copies: each spawned pipe task moves its own in.
    let input_log_err = input.to_string();

    // Stream both pipes to the UI **and** install.log (the complete plugin
    // install output must survive for post-hoc diagnosis — stderr included).
    if let Some(out) = stdout {
        let app = app.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log(&format!("[plugin:{input_log}] {line}"));
                config::install_log_line(&format!("[plugin:{input_log}] {line}"));
                let _ = app.emit("dsh://plugin", line);
            }
        });
    }
    if let Some(err) = stderr {
        let app = app.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                config::install_log_line(&format!("[plugin:{input_log_err}:stderr] {line}"));
                let _ = app.emit("dsh://plugin", line);
            }
        });
    }

    // 10 minutes per attempt: a fresh npx download plus a GitHub tarball
    // can be slow on a poor connection.
    let status = match tokio::time::timeout(std::time::Duration::from_secs(600), child.wait())
        .await
    {
        Err(_) => {
            return PluginRun::Fatal(
                "The plugin install timed out after 10 minutes.".to_string(),
            )
        }
        Ok(Err(e)) => {
            return PluginRun::Fatal(format!("The plugin command failed to run: {e}"))
        }
        Ok(Ok(s)) => s,
    };

    // Give the last output lines a moment to reach the UI before reporting.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    if status.success() {
        PluginRun::Success
    } else {
        PluginRun::FailedExit(status.code())
    }
}

// ---------------------------------------------------------------------------
// Web profile helpers (plugin idempotency / removal)
// ---------------------------------------------------------------------------

/// The only two plugins the launcher's install UI manages (remove + idempotency
/// are scoped to these to avoid touching unrelated packages in the profile).
const SUPPORTED_PLUGIN_NAMES: [&str; 2] = ["dsh-additive", "dsh-convfusion"];

/// Path to the `web` profile's package.json (DSH_HOME defaults to `~/.dsh`).
fn web_profile_package_json() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".dsh/profiles/web/package.json"))
}

/// Parse a plugin `input` string and return the package name if it maps to one
/// of the launcher's two supported plugins. Recognises npm names, github
/// `owner/repo#ref` forms, full https:// URLs, and wrapped npx commands.
fn supported_plugin_name(input: &str) -> Option<String> {
    let s = input.to_ascii_lowercase();
    if s.contains("convfusion/dsh-additive")
        || s.contains("github:convfusion/dsh-additive")
        || s.trim() == "dsh-additive"
    {
        return Some("dsh-additive".into());
    }
    if s.contains("convfusion/convfusion-dsh")
        || s.contains("github:convfusion/convfusion-dsh")
        || s.trim() == "dsh-convfusion"
    {
        return Some("dsh-convfusion".into());
    }
    None
}

/// Read dependency keys from the web profile's package.json (empty if the
/// profile doesn't exist yet or can't be parsed).
fn read_profile_deps() -> Vec<String> {
    let Some(path) = web_profile_package_json() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("dependencies")
        .and_then(|d| d.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

/// Installed status of the launcher's supported plugins.
#[derive(Serialize)]
pub struct PluginStatus {
    pub name: String,
    pub installed: bool,
}

#[tauri::command]
pub fn plugin_status() -> Vec<PluginStatus> {
    let deps = read_profile_deps();
    SUPPORTED_PLUGIN_NAMES
        .iter()
        .map(|n| PluginStatus {
            name: (*n).to_string(),
            installed: deps.iter().any(|d| d == n),
        })
        .collect()
}

/// Build npx args for `dsh plugin --profile web remove <pkg>`.
fn plugin_remove_npx_args(pkg: &str) -> Vec<String> {
    vec![
        "@deepseek-ai/dsh".into(),
        "plugin".into(),
        "--profile".into(),
        "web".into(),
        "remove".into(),
        pkg.to_string(),
    ]
}

/// Remove one of the launcher's supported plugins from the `web` profile.
/// Streaming output goes to `dsh://plugin` (same channel as install) so the UI
/// already knows how to display it. Rejected for any other package name — we
/// don't touch user-installed plugins we don't recognise.
#[tauri::command]
pub async fn remove_dsh_plugin(
    state: State<'_, AppState>,
    app: AppHandle,
    name: String,
) -> Result<String, String> {
    let pkg = name.trim().to_string();
    if !SUPPORTED_PLUGIN_NAMES.contains(&pkg.as_str()) {
        return Err(format!(
            "Removal is only supported for the launcher's own plugins ({}).",
            SUPPORTED_PLUGIN_NAMES.join(", ")
        ));
    }

    let deps = read_profile_deps();
    if !deps.iter().any(|d| d == &pkg) {
        log(&format!("plugin remove: {pkg} is not installed — nothing to do"));
        return Ok("not-installed".into());
    }

    let node = state
        .node()
        .ok_or("No compatible Node.js runtime is available.")?;
    let npx_cli = crate::runtime::detector::npx_cli_for(&node.path)
        .ok_or_else(|| format!("Cannot locate npx for Node at {}.", node.path.display()))?;
    let npm_cli = {
        let sibling = npx_cli.with_file_name("npm-cli.js");
        if sibling.exists() {
            sibling
        } else {
            crate::runtime::detector::npm_cli_for(&node.path).unwrap_or(sibling)
        }
    };
    let pnpm_dir = ensure_pnpm(&node, &npm_cli, &app).await?;
    if let Some(dir) = pnpm_dir.as_ref() {
        log(&format!("plugin remove: pnpm from {}", dir.display()));
    }

    let args = plugin_remove_npx_args(&pkg);
    log(&format!(
        "plugin remove command: {} {}",
        npx_cli.display(),
        args.join(" ")
    ));
    config::install_log_line(&format!(
        "$ node {} {}",
        npx_cli.display(),
        args.join(" ")
    ));

    match run_plugin_once(
        &node,
        &npx_cli,
        &args,
        &app,
        &format!("remove {pkg}"),
        pnpm_dir.as_deref(),
    )
    .await
    {
        PluginRun::Success => {
            log(&format!("plugin {pkg} removed"));
            Ok("removed".into())
        }
        PluginRun::FailedExit(code) => Err(format!(
            "Removing {pkg} failed (exit code {code:?}) — see the log above."
        )),
        PluginRun::Fatal(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Port helper for the "Use another port" flow
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn suggest_ports(preferred: u16) -> Vec<u16> {
    health::suggest_ports(preferred, 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args_ok(input: &str) -> Vec<String> {
        plugin_npx_args(input).unwrap_or_else(|e| panic!("{input:?} rejected: {e}"))
    }

    fn args_err(input: &str) -> String {
        plugin_npx_args(input).unwrap_err()
    }

    #[test]
    fn npm_names_get_standard_wrapper() {
        for name in ["@rose43/dsh-file", "dsh1024@latest", "dsh-file"] {
            let args = args_ok(name);
            assert!(args.starts_with(&[
                "-y".to_string(),
                "--package".to_string(),
                "@deepseek-ai/dsh".to_string(),
                "dsh".to_string(),
                "plugin".to_string(),
                "--profile".to_string(),
                "web".to_string(),
                "add".to_string(),
            ]), "{name}: {args:?}");
            assert_eq!(args.last().unwrap(), name, "{name}: {args:?}");
        }
    }

    #[test]
    fn github_references_are_accepted() {
        for ref_name in [
            "github:LoftyTao/dsh-ui-workbench#v0.3.0",
            "github:dcrzsy/dsh-enhance-tool",
            "github:owner/repo#feature/branch",
        ] {
            let args = args_ok(ref_name);
            assert_eq!(args.last().unwrap(), ref_name);
        }
        // Missing the owner/repo split, or extra segments, is invalid.
        args_err("github:onlyowner");
        args_err("github:a/b/c");
        args_err("github:owner/");
    }

    #[test]
    fn local_paths_are_accepted_and_home_expanded() {
        let args = args_ok("/Users/foo/plugins/my-plugin");
        assert_eq!(args.last().unwrap(), "/Users/foo/plugins/my-plugin");

        if let Some(home) = dirs::home_dir() {
            let args = args_ok("~/my-plugin");
            assert_eq!(args.last().unwrap(), &home.join("my-plugin").to_string_lossy());
        }
    }

    #[test]
    fn shell_metacharacters_are_rejected() {
        for bad in [
            "foo; rm -rf /",
            "foo | bar",
            "$(reboot)",
            "foo && bar",
            "foo > out",
            "foo `id`",
            "foo\nbar",
            "foo'bar",
            "foo\"bar",
        ] {
            let _ = args_err(bad);
        }
    }

    #[test]
    fn full_npx_commands_run_as_is() {
        let input = "npx -y --package @deepseek-ai/dsh dsh plugin --profile web add github:LoftyTao/dsh-ui-workbench#v0.3.0";
        let args = args_ok(input);
        let expected: Vec<String> = input
            .split_whitespace()
            .skip(1)
            .map(str::to_string)
            .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn missing_profile_is_filled_in_for_plugin_commands() {
        // The one-click links an earlier build offered used this shape, and
        // `dsh plugin` rejects it: "required option '--profile <name>' not
        // specified". The launcher boots the `web` profile, so supply it.
        let args = args_ok("npx @deepseek-ai/dsh plugin add https://github.com/ConvFusion/DSH-additive");
        assert_eq!(
            args,
            vec![
                "@deepseek-ai/dsh",
                "plugin",
                "--profile",
                "web",
                "add",
                "https://github.com/ConvFusion/DSH-additive",
            ]
        );

        // The explicit `--package … dsh` spelling is handled too.
        let args = args_ok("npx -y --package @deepseek-ai/dsh dsh plugin add some-plugin");
        assert_eq!(
            args,
            vec![
                "-y",
                "--package",
                "@deepseek-ai/dsh",
                "dsh",
                "plugin",
                "--profile",
                "web",
                "add",
                "some-plugin",
            ]
        );

        // An explicit profile is never overridden, and non-plugin commands
        // are left untouched.
        let args = args_ok("npx @deepseek-ai/dsh plugin --profile tui add some-plugin");
        assert_eq!(
            args,
            vec!["@deepseek-ai/dsh", "plugin", "--profile", "tui", "add", "some-plugin"]
        );
        let args = args_ok("npx @deepseek-ai/dsh web --port 3080");
        assert_eq!(args, vec!["@deepseek-ai/dsh", "web", "--port", "3080"]);
    }

    #[test]
    fn full_npx_commands_must_target_dsh() {
        args_err("npx -y --package some-other-pkg do stuff");
        args_err("npx");
        args_err("npx ");
    }

    #[test]
    fn npm_names_starting_with_npx_are_not_commands() {
        let args = args_ok("npx-tools");
        assert_eq!(args.last().unwrap(), "npx-tools");
    }

    #[test]
    fn supported_plugin_names_are_recognised_in_every_input_form() {
        // Full npx command (what the one-click cards fill in).
        assert_eq!(
            supported_plugin_name(
                "npx @deepseek-ai/dsh plugin --profile web add https://github.com/ConvFusion/DSH-additive"
            ),
            Some("dsh-additive".into())
        );
        // Bare repo URL / github shorthand / npm name.
        assert_eq!(
            supported_plugin_name("https://github.com/ConvFusion/DSH-additive"),
            Some("dsh-additive".into())
        );
        assert_eq!(
            supported_plugin_name("github:ConvFusion/DSH-additive"),
            Some("dsh-additive".into())
        );
        assert_eq!(supported_plugin_name("dsh-additive"), Some("dsh-additive".into()));
        // The research plugin's package name differs from its repo name.
        assert_eq!(
            supported_plugin_name("https://github.com/ConvFusion/ConvFusion-dsh"),
            Some("dsh-convfusion".into())
        );
        assert_eq!(
            supported_plugin_name("dsh-convfusion"),
            Some("dsh-convfusion".into())
        );
        // Anything else is left alone: no idempotency check, no removal.
        assert_eq!(supported_plugin_name("@rose43/dsh-file"), None);
        assert_eq!(supported_plugin_name("github:owner/other-plugin"), None);
    }

    #[test]
    fn remove_args_target_the_web_profile() {
        assert_eq!(
            plugin_remove_npx_args("dsh-additive"),
            vec![
                "@deepseek-ai/dsh",
                "plugin",
                "--profile",
                "web",
                "remove",
                "dsh-additive",
            ]
        );
    }
}
