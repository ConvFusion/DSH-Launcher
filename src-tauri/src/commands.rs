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
    let on_tail: Box<dyn Fn(String) + Send> = Box::new({
        let app = app.clone();
        move |tail| {
            let _ = app.emit(
                "dsh://env",
                EnvProgress::fail("dsh", "npm is reporting errors…", Some(tail)),
            );
        }
    });
    let target = state.dsh_target_dir();
    let version =
        runtime::installer::install_dsh(&node.path, &target, Some(on_tail)).await?;
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
        let tokens: Vec<String> = input.split_whitespace().map(str::to_string).collect();
        if tokens.len() < 2 {
            return Err("The npx command is missing arguments.".into());
        }
        if !input.contains("@deepseek-ai/dsh") {
            return Err(
                "The command must target the @deepseek-ai/dsh package, e.g. `npx -y --package @deepseek-ai/dsh dsh plugin --profile web add …`."
                    .into(),
            );
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

        match run_plugin_once(&node, &npx_cli, &npx_args, &app, &input).await {
            PluginRun::Success => return Ok(input),
            PluginRun::FailedExit(code) => {
                last_exit = code;
            }
            PluginRun::Fatal(e) => return Err(e),
        }
    }

    Err(format!(
        "The plugin install failed after {MAX_ATTEMPTS} attempts (exit code {:?}) — see the log above.",
        last_exit
    ))
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
    cmd.env("PATH", format!("{}{}{old_path}", node_dir.display(), sep));
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return PluginRun::Fatal(format!("Could not start the plugin command: {e}")),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Stream both pipes to the UI.
    if let Some(out) = stdout {
        let app = app.clone();
        let input_log = input.to_string();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                log(&format!("[plugin:{input_log}] {line}"));
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
}
