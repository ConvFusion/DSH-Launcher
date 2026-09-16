//! Process manager for the DeepSeek Harness service.
//!
//! States: `Stopped → Starting → Running → Stopping → Stopped`, plus `Error`.
//!
//! The manager is deliberately signal-based: stop() talks to the OS by pid
//! (SIGTERM/TerminateProcess), while the monitor task that owns the tokio
//! `Child` only reacts to the process exiting. This avoids fighting over
//! ownership of the `Child` between concurrent commands.

use super::health;
use crate::config::{self, log, now_stamp, ProcessStateFile};
use crate::runtime::detector;
use crate::runtime::{DshInfo, NodeInfo};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

pub const READY_TIMEOUT: Duration = Duration::from_secs(120);
const TAIL_LINES: usize = 300;

/// How long to wait for the harness's `?token=…` banner before opening a
/// browser anyway. Readiness only means "the port answers HTTP" (a 401 does
/// count), and the banner is printed around that same moment — usually a
/// fraction of a second later. Opening the bare URL in the meantime lands the
/// user on the "unauthorized" page.
const TOKEN_WAIT: Duration = Duration::from_millis(2500);

/// How long `node <dsh>/lib/bin.js --version` may take before the runtime is
/// considered unusable. A healthy run answers in well under a second; this is
/// only a guard against a wedged binary.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the installed DSH CLI once with `--version` and require a version line
/// back.
///
/// This is the empirical half of the runtime check (the static half is
/// [`detector::node_compatible`]). `@deepseek-ai/dsh`'s `lib/bin.js` ends with
/// `if (import.meta.main) await runCli();`, and `import.meta.main` exists only
/// from Node 22.18 / 24.2 — on an older runtime the CLI runs **nothing**,
/// prints nothing and exits 0, so the service "started" and vanished with no
/// diagnostic whatsoever. Requiring output here turns that silent no-op into
/// an actionable error, whatever the underlying cause (a too-old Node, a
/// half-finished install, a broken native module).
///
/// Returns the reported version, or an explanation of what went wrong.
async fn probe_dsh_cli(
    node: &std::path::Path,
    bin_js: &std::path::Path,
    node_dir: &std::path::Path,
) -> Result<String, String> {
    // Same PATH fix-up as the real spawn: npm lifecycle helpers and DSH's own
    // child processes resolve `node` by name.
    let sep = if cfg!(windows) { ";" } else { ":" };
    let old_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}{}{old_path}", node_dir.display(), sep);

    let fut = tokio::process::Command::new(node)
        .arg(bin_js)
        .arg("--version")
        .env("PATH", new_path)
        .stdin(std::process::Stdio::null())
        .output();
    let out = match tokio::time::timeout(PREFLIGHT_TIMEOUT, fut).await {
        Err(_) => {
            return Err(format!(
                "`{} {} --version` did not answer within {}s.",
                node.display(),
                bin_js.display(),
                PREFLIGHT_TIMEOUT.as_secs()
            ))
        }
        Ok(Err(e)) => return Err(format!("Could not execute {}: {e}", node.display())),
        Ok(Ok(o)) => o,
    };

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !stdout.is_empty() {
        return Ok(stdout.lines().next().unwrap_or_default().trim().to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(format!(
        "`node bin.js --version` produced no version (exit code {:?}){}",
        out.status.code(),
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}



#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcSnapshot {
    pub state: ProcessState,
    pub pid: Option<u32>,
    pub host: String,
    pub port: u16,
    pub url: String,
    pub error: Option<String>,
    pub error_details: Option<String>,
    pub started_at: Option<String>,
    /// Output tail of the harness process (for error details).
    pub output_tail: Vec<String>,
    /// True when the instance was started outside the launcher (e.g. a
    /// manual `dsh web` in a terminal) and adopted by probing the port.
    pub external: bool,
}

/// Result of a start/restart attempt, structured so the UI can act on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartOutcome {
    pub ok: bool,
    /// "ready" | "already_running" | "port_in_use" | "error"
    pub kind: String,
    pub port: Option<u16>,
    pub suggestions: Vec<u16>,
    pub message: Option<String>,
    pub details: Option<String>,
}

impl StartOutcome {
    pub fn ready(port: u16) -> Self {
        Self {
            ok: true,
            kind: "ready".into(),
            port: Some(port),
            suggestions: vec![],
            message: None,
            details: None,
        }
    }
    pub fn already_running(port: u16) -> Self {
        Self {
            ok: true,
            kind: "already_running".into(),
            port: Some(port),
            suggestions: vec![],
            message: Some("DeepSeek Harness is already running.".into()),
            details: None,
        }
    }
    pub fn port_in_use(port: u16, suggestions: Vec<u16>) -> Self {
        Self {
            ok: false,
            kind: "port_in_use".into(),
            port: Some(port),
            suggestions,
            message: Some(format!("Port {port} is already in use by another program.")),
            details: None,
        }
    }
    pub fn error(message: String, details: Option<String>) -> Self {
        Self {
            ok: false,
            kind: "error".into(),
            port: None,
            suggestions: vec![],
            message: Some(message),
            details,
        }
    }
}

struct Inner {
    state: ProcessState,
    pid: Option<u32>,
    host: String,
    port: u16,
    /// Full URL with auth token, parsed from dsh web output (e.g.
    /// "http://127.0.0.1:3080/?token=xxx"). None for older dsh versions or
    /// externally-adopted instances where we never saw the startup banner.
    token_url: Option<String>,
    error: Option<String>,
    error_details: Option<String>,
    started_at: Option<String>,
    tail: VecDeque<String>,
    /// Set while a stop was requested (so the monitor doesn't raise an error).
    stop_requested: bool,
    /// True when the running instance was started outside the launcher and
    /// adopted by probing the port; such an instance is shown as Running but
    /// we never spawned it.
    external: bool,
}

impl Inner {
    fn new(host: String, port: u16) -> Self {
        Self {
            state: ProcessState::Stopped,
            pid: None,
            host,
            port,
            token_url: None,
            error: None,
            error_details: None,
            started_at: None,
            tail: VecDeque::new(),
            stop_requested: false,
            external: false,
        }
    }

    fn snapshot(&self) -> ProcSnapshot {
        let url = self.token_url.clone().unwrap_or_else(|| {
            format!("http://{}:{}", self.host, self.port)
        });
        ProcSnapshot {
            state: self.state,
            pid: self.pid,
            host: self.host.clone(),
            port: self.port,
            url,
            error: self.error.clone(),
            error_details: self.error_details.clone(),
            started_at: self.started_at.clone(),
            output_tail: self.tail.iter().rev().take(15).cloned().collect::<Vec<_>>().into_iter().rev().collect(),
            external: self.external,
        }
    }
}

#[derive(Clone)]
pub struct ProcessManager {
    inner: Arc<Mutex<Inner>>,
}

impl ProcessManager {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new(
                host.to_string(),
                port,
            ))),
        }
    }

    pub fn snapshot(&self) -> ProcSnapshot {
        self.inner.lock().unwrap().snapshot()
    }

    /// Update the configured host/port (from the settings page).
    pub fn set_target(&self, host: &str, port: u16) {
        let mut g = self.inner.lock().unwrap();
        g.host = host.to_string();
        g.port = port;
    }

    /// Wait up to [`TOKEN_WAIT`] for the `?token=…` URL to be parsed from the
    /// harness output, returning it as soon as it appears.
    ///
    /// A browser must never be opened on the bare `http://host:port` while the
    /// token is still on its way: the harness answers `401` without it, so the
    /// user would land on an error page for a service that is actually up.
    /// Returns `None` on timeout (callers then fall back to the bare URL).
    pub async fn wait_for_token_url(&self) -> Option<String> {
        // An instance started outside the launcher has no output pipe of ours,
        // so no token can ever arrive — never make the user wait for nothing.
        if self.inner.lock().unwrap().external {
            return None;
        }
        let deadline = Instant::now() + TOKEN_WAIT;
        loop {
            if let Some(url) = self.inner.lock().unwrap().token_url.clone() {
                return Some(url);
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn emit_status(app: &AppHandle) {
        // Resolved through the manager map — see state.rs.
        let Some(state) = app.try_state::<crate::state::AppState>() else {
            return;
        };
        state.push_status(app);
    }

    fn tail_details(g: &Inner) -> Option<String> {
        let lines: Vec<String> = g.tail.iter().rev().take(12).cloned().collect();
        if lines.is_empty() {
            None
        } else {
            Some(lines.into_iter().rev().collect::<Vec<_>>().join("\n"))
        }
    }

    /// Adopt a running instance:
    ///
    /// 1. a previously started instance recorded in `state.json` (pid alive
    ///    and HTTP responds), or
    /// 2. **anything already serving HTTP on the configured port** — e.g. a
    ///    `dsh web` started manually in a terminal. Such an instance is
    ///    adopted as `external`: shown as Running, never spawned by us.
    ///
    /// Returns true when we are now Running.
    pub async fn try_adopt(&self, app: &AppHandle) -> bool {
        // 1. Managed instance from a previous launcher run.
        if let Some(st) = config::read_process_state() {
            if health::process_alive(st.pid) && health::http_ok(&st.host, st.port).await {
                let mut g = self.inner.lock().unwrap();
                g.host = st.host.clone();
                g.port = st.port;
                g.pid = Some(st.pid);
                g.started_at = Some(st.started_at.clone());
                g.token_url = st.token_url.clone();
                g.external = false;
                g.state = ProcessState::Running;
                g.error = None;
                g.error_details = None;
                drop(g);
                log(&format!("adopted running instance pid={} port={}", st.pid, st.port));
                Self::emit_status(app);
                return true;
            }
            config::clear_process_state();
        }

        // 2. External instance: probe the configured host:port for an HTTP
        //    service that we did not start (no state.json record).
        let (host, port) = {
            let g = self.inner.lock().unwrap();
            (g.host.clone(), g.port)
        };
        if health::http_ok(&host, port).await {
            let pid = health::listener_pid(&host, port);
            let mut g = self.inner.lock().unwrap();
            g.host = host.clone();
            g.port = port;
            g.pid = pid;
            g.started_at = None;
            g.external = true;
            g.state = ProcessState::Running;
            g.error = None;
            g.error_details = None;
            drop(g);
            log(&format!(
                "adopted externally running service on port {port} (pid {pid:?})"
            ));
            Self::emit_status(app);
            return true;
        }

        false
    }

    /// Spawn the harness, wait for HTTP readiness, and report.
    ///
    /// Browser opening is handled by the caller once we return "ready".
    pub async fn start(
        &self,
        app: &AppHandle,
        node: &NodeInfo,
        dsh: &DshInfo,
    ) -> StartOutcome {
        let (state, pid) = {
            let g = self.inner.lock().unwrap();
            (g.state, g.pid)
        };
        if matches!(state, ProcessState::Starting | ProcessState::Running)
            && pid.map(health::process_alive).unwrap_or(false)
        {
            let port = self.inner.lock().unwrap().port;
            return StartOutcome::already_running(port);
        }

        // Maybe a previous launcher run left a live instance behind.
        if self.try_adopt(app).await {
            let port = self.inner.lock().unwrap().port;
            return StartOutcome::already_running(port);
        }

        let (host, port) = {
            let g = self.inner.lock().unwrap();
            (g.host.clone(), g.port)
        };
        if health::port_in_use(&host, port) {
            let suggestions = health::suggest_ports(port, 3);
            log(&format!("port {port} busy, suggestions: {suggestions:?}"));
            return StartOutcome::port_in_use(port, suggestions);
        }

        let node_bin = node.path.clone();
        let node_dir = node_bin.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf();
        let bin_js = dsh
            .path
            .join("node_modules")
            .join("@deepseek-ai/dsh")
            .join("lib")
            .join("bin.js");
        if !bin_js.exists() {
            return StartOutcome::error(
                "DeepSeek Harness is not installed yet.".into(),
                None,
            );
        }

        // Preflight: prove this Node.js can actually execute the installed
        // DSH before committing to a long-lived service (see
        // `probe_dsh_cli`). Cheap — one `--version` run — and it converts the
        // silent "started, then exited 0 with no output" failure into a
        // message that names the cause.
        if let Err(detail) = probe_dsh_cli(&node_bin, &bin_js, &node_dir).await {
            let message = if detector::node_compatible(&node.version) {
                "Unable to start DeepSeek Harness.".to_string()
            } else {
                format!(
                    "Node.js v{} is too old for DeepSeek Harness (needs {}). \
                     Install a newer Node.js or let DSH Launcher download its own runtime.",
                    node.version,
                    detector::MIN_NODE_LABEL
                )
            };
            log(&format!(
                "preflight failed for node v{}: {detail}",
                node.version
            ));
            {
                let mut g = self.inner.lock().unwrap();
                g.state = ProcessState::Error;
                g.error = Some(message.clone());
                g.error_details = Some(detail.clone());
            }
            Self::emit_status(app);
            return StartOutcome::error(message, Some(detail));
        }

        // Mark starting.
        {
            let mut g = self.inner.lock().unwrap();
            g.state = ProcessState::Starting;
            g.error = None;
            g.error_details = None;
            g.tail.clear();
            g.token_url = None;
            g.stop_requested = false;
            g.started_at = Some(now_stamp());
            // This process is ours from here on. Without this, an instance that
            // was once adopted from outside (`external = true`, never cleared
            // by `stop`) would keep the UI showing "external instance" and
            // suppress the token wait for a process we actually own.
            g.external = false;
        }
        Self::emit_status(app);
        log(&format!(
            "starting: node={} dsh={} port={port}",
            node_bin.display(),
            bin_js.display()
        ));

        // Build the child command: node <dsh>/lib/bin.js web --host --port --no-open
        let mut cmd = tokio::process::Command::new(&node_bin);
        cmd.arg(&bin_js)
            .arg("web")
            .arg("--host")
            .arg(&host)
            .arg("--port")
            .arg(port.to_string())
            .arg("--no-open");
        // Prepend our node directory to PATH so child tools resolve consistently.
        let sep = if cfg!(windows) { ";" } else { ":" };
        let old_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}{}{old_path}", node_dir.display(), sep);
        cmd.env("PATH", new_path);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            // The child is a headless GUI service: run it without a console.
            // CREATE_NO_WINDOW keeps it out of a (new) console window — the
            // parent is a GUI app — and CREATE_NEW_PROCESS_GROUP puts it in
            // its own process group. The old code wrote 0x00000010 here,
            // which is CREATE_NEW_CONSOLE, not DETACHED_PROCESS: it actively
            // created the persistent node.exe console window.
            use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let mut g = self.inner.lock().unwrap();
                g.state = ProcessState::Error;
                g.error = Some("Unable to start DeepSeek Harness.".into());
                g.error_details = Some(format!("Could not start the Node.js process: {e}"));
                drop(g);
                Self::emit_status(app);
                return StartOutcome::error(
                    "Unable to start DeepSeek Harness.".into(),
                    Some(format!("Could not start the Node.js process: {e}")),
                );
            }
        };
        let pid = child.id().unwrap_or(0);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        {
            let mut g = self.inner.lock().unwrap();
            g.pid = Some(pid);
        }
        config::write_process_state(&ProcessStateFile {
            pid,
            host: host.clone(),
            port,
            started_at: now_stamp(),
            token_url: None,
        });

        // Output pump: both pipes → harness.log + in-memory tail. The app
        // handle is passed on so the tasks can push a status update the moment
        // the token URL is discovered (see `reader_task`).
        let inner = Arc::clone(&self.inner);
        if let Some(out) = stdout {
            tokio::spawn(reader_task(out, inner.clone(), app.clone()));
        }
        if let Some(err) = stderr {
            tokio::spawn(reader_task_err(err, inner, app.clone()));
        }

        // Monitor: owns the child; records the exit.
        let inner2 = Arc::clone(&self.inner);
        let app2 = app.clone();
        let host2 = host.clone();
        let port2 = port;
        tokio::spawn(async move {
            let status = child.wait().await;
            let exit_code = status.as_ref().ok().and_then(|s| s.code());
            let succeeded = status.as_ref().map(|s| s.success()).unwrap_or(false);
            config::clear_process_state();
            let mut g = inner2.lock().unwrap();
            g.pid = None;
            let stop_requested = g.stop_requested;
            if !g.stop_requested {
                let expected = succeeded;
                if !expected {
                    g.state = ProcessState::Error;
                    g.error = Some(
                        "DeepSeek Harness stopped unexpectedly.".into(),
                    );
                    g.error_details =
                        Self::tail_details(&g).or(Some(format!("exit code: {exit_code:?}")));
                } else {
                    g.state = ProcessState::Stopped;
                }
            }
            g.stop_requested = false;
            drop(g);
            log(&format!(
                "harness process exited (code {exit_code:?}, succeeded={succeeded}, stop_requested={stop_requested})"
            ));
            Self::emit_status(&app2);
            // Silence unused warnings on some platforms.
            let _ = (&host2, &port2);
        });

        // Wait for HTTP readiness (no sleeps-as-guesses).
        let ready = match health::wait_ready(&host, port, READY_TIMEOUT).await {
            Ok(()) => true,
            Err(e) => {
                // Distinguish "user stopped us" from "process died" from "too slow".
                let (user_stop, still_alive) = {
                    let g = self.inner.lock().unwrap();
                    (g.stop_requested, g.pid.map(health::process_alive).unwrap_or(false))
                };
                if user_stop {
                    // stop() already set state; just report.
                } else if !still_alive {
                    let details = {
                        let g = self.inner.lock().unwrap();
                        Self::tail_details(&g)
                    };
                    let mut g = self.inner.lock().unwrap();
                    g.state = ProcessState::Error;
                    g.error = Some("Unable to start DeepSeek Harness.".into());
                    g.error_details = Some(
                        details.unwrap_or_else(|| "The process exited before the service became ready.".into()),
                    );
                    drop(g);
                    Self::emit_status(app);
                } else {
                    let _ = self.stop(app).await;
                }
                let _ = e;
                false
            }
        };

        if ready {
            let mut g = self.inner.lock().unwrap();
            g.state = ProcessState::Running;
            g.error = None;
            g.error_details = None;
            drop(g);
            log(&format!("harness ready at http://{host}:{port}"));
            Self::emit_status(app);
            StartOutcome::ready(port)
        } else {
            let (message, details) = {
                let g = self.inner.lock().unwrap();
                (g.error.clone(), g.error_details.clone())
            };
            Self::emit_status(app);
            StartOutcome::error(
                message.unwrap_or_else(|| "Unable to start DeepSeek Harness.".into()),
                details,
            )
        }
    }

    /// Graceful stop: SIGTERM (or TerminateProcess), then force kill after 5s.
    pub async fn stop(&self, app: &AppHandle) -> Result<(), String> {
        let (pid, external) = {
            let g = self.inner.lock().unwrap();
            (g.pid, g.external)
        };
        let Some(pid) = pid else {
            let mut g = self.inner.lock().unwrap();
            if external && g.state == ProcessState::Running {
                // Adopted externally but no pid could be resolved (lsof /
                // netstat unavailable): we cannot stop it — say so instead
                // of pretending the service is gone.
                drop(g);
                return Err(
                    "This instance was started outside DSH Launcher and its process \
                     could not be identified, so the launcher cannot stop it. \
                     Stop it in the terminal where you launched it."
                        .into(),
                );
            }
            if g.state != ProcessState::Stopped {
                g.state = ProcessState::Stopped;
            }
            Self::emit_status(app);
            return Ok(());
        };
        if !health::process_alive(pid) {
            config::clear_process_state();
            let mut g = self.inner.lock().unwrap();
            g.pid = None;
            g.state = ProcessState::Stopped;
            g.stop_requested = false;
            drop(g);
            Self::emit_status(app);
            return Ok(());
        }

        {
            let mut g = self.inner.lock().unwrap();
            g.state = ProcessState::Stopping;
            g.stop_requested = true;
        }
        Self::emit_status(app);
        log(&format!("stopping harness pid={pid}"));
        health::signal_terminate(pid);

        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !health::process_alive(pid) {
                break;
            }
        }
        if health::process_alive(pid) {
            log(&format!("harness pid={pid} did not exit, force killing"));
            health::signal_kill(pid);
        }
        config::clear_process_state();
        let mut g = self.inner.lock().unwrap();
        g.pid = None;
        g.state = ProcessState::Stopped;
        g.error = None;
        g.stop_requested = false;
        drop(g);
        Self::emit_status(app);
        Ok(())
    }

    /// Periodic health poll, called from the background watcher:
    ///
    /// * if we think the harness is Running but nothing answers HTTP on the
    ///   configured port, mark it Stopped (it died or was killed outside the
    ///   launcher);
    /// * if something answers HTTP but we are not Running, adopt it (a
    ///   service started manually while the launcher was open).
    ///
    /// Emits a status update only when the state actually changed.
    pub async fn poll_status(&self, app: &AppHandle) {
        let (state, host, port) = {
            let g = self.inner.lock().unwrap();
            (g.state, g.host.clone(), g.port)
        };

        let ok = health::http_ok(&host, port).await;

        match (state, ok) {
            // Running but the endpoint stopped answering → someone stopped
            // or killed it outside the launcher (or it crashed).
            (ProcessState::Running, false) => {
                let mut g = self.inner.lock().unwrap();
                if g.state == ProcessState::Running && !g.stop_requested {
                    g.state = ProcessState::Stopped;
                    g.pid = None;
                    g.error = None;
                    g.error_details = None;
                    config::clear_process_state();
                    log(&format!("health poll: harness at {host}:{port} is gone"));
                    drop(g);
                    Self::emit_status(app);
                }
            }
            // Not running but the endpoint answers → an external instance
            // was started (e.g. `dsh web` in a terminal) while we were open.
            (ProcessState::Stopped | ProcessState::Error, true) => {
                if self.try_adopt(app).await {
                    log(&format!(
                        "health poll: adopted service that appeared on {host}:{port}"
                    ));
                }
            }
            _ => {}
        }
    }
}

async fn reader_task(
    pipe: tokio::process::ChildStdout,
    inner: Arc<Mutex<Inner>>,
    app: AppHandle,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        config::harness_log_line(&line);
        let maybe_token = {
            let mut g = inner.lock().unwrap();
            g.tail.push_back(line.clone());
            while g.tail.len() > TAIL_LINES {
                g.tail.pop_front();
            }
            // Parse "dsh web: http://.../?token=..." printed on startup.
            if g.token_url.is_none() {
                if let Some(url) = extract_token_url(&line) {
                    g.token_url = Some(url.clone());
                    Some((g.pid, g.host.clone(), g.port, g.started_at.clone(), url))
                } else {
                    None
                }
            } else {
                None
            }
        };
        // Persist the token URL so restarting the launcher keeps the URL valid.
        if let Some((pid, host, port, started_at, token_url)) = maybe_token {
            if let Some(pid) = pid {
                let started_at = started_at.unwrap_or_else(now_stamp);
                config::write_process_state(&ProcessStateFile {
                    pid,
                    host,
                    port,
                    started_at,
                    token_url: Some(token_url),
                });
            }
            // The token is exactly what the UI displays and opens, and it
            // usually arrives *after* the "ready" status was already pushed
            // (readiness only means the port answers). Without this push the
            // home page keeps showing the bare `http://host:port` until the
            // user presses Refresh.
            ProcessManager::emit_status(&app);
        }
    }
}

async fn reader_task_err(
    pipe: tokio::process::ChildStderr,
    inner: Arc<Mutex<Inner>>,
    app: AppHandle,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        config::harness_log_line(&line);
        let maybe_token = {
            let mut g = inner.lock().unwrap();
            g.tail.push_back(line.clone());
            while g.tail.len() > TAIL_LINES {
                g.tail.pop_front();
            }
            // Also scan stderr for the token URL (npm warnings and other noise
            // go to stderr, but dsh itself prints the banner to stdout; check
            // both to be safe across versions).
            if g.token_url.is_none() {
                if let Some(url) = extract_token_url(&line) {
                    g.token_url = Some(url.clone());
                    Some((g.pid, g.host.clone(), g.port, g.started_at.clone(), url))
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((pid, host, port, started_at, token_url)) = maybe_token {
            if let Some(pid) = pid {
                let started_at = started_at.unwrap_or_else(now_stamp);
                config::write_process_state(&ProcessStateFile {
                    pid,
                    host,
                    port,
                    started_at,
                    token_url: Some(token_url),
                });
            }
            // Same push as in `reader_task`: make the token URL appear in the
            // UI without a manual Refresh.
            ProcessManager::emit_status(&app);
        }
    }
}

/// Extract a `http(s)://…/?token=…` URL from a dsh startup banner line.
///
/// Example input:
///   `dsh web: http://127.0.0.1:3080/?token=4Ob4Ii4f0565PO1SgLJFzBkqo02qdw_GkpSrOTI1RAw`
/// Returns the URL substring, or None if the line does not contain one.
fn extract_token_url(line: &str) -> Option<String> {
    // Find "http://" or "https://" in the line.
    let start = line.find("http://").or_else(|| line.find("https://"))?;
    // The URL runs until whitespace (or end of string).
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace())
        .unwrap_or(rest.len());
    let url = &rest[..end];
    // Must contain the token query parameter; otherwise it's an unrelated URL.
    if url.contains("?token=") || url.contains("&token=") {
        Some(url.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_token_url_from_dsh_banner() {
        let line = "dsh web: http://127.0.0.1:3080/?token=4Ob4Ii4f0565PO1SgLJFzBkqo02qdw_GkpSrOTI1RAw";
        assert_eq!(
            extract_token_url(line),
            Some("http://127.0.0.1:3080/?token=4Ob4Ii4f0565PO1SgLJFzBkqo02qdw_GkpSrOTI1RAw".into())
        );
    }

    #[test]
    fn ignores_plain_urls_without_token() {
        assert_eq!(extract_token_url("dsh web: http://127.0.0.1:3080/"), None);
        assert_eq!(extract_token_url("listening on http://127.0.0.1:3080"), None);
        assert_eq!(extract_token_url("[dsh-file] FileManagerGateway constructed, root=/Users/x/.dsh/profiles/web"), None);
    }

    #[test]
    fn extracts_url_even_with_trailing_noise() {
        let line = "dsh web: http://127.0.0.1:3080/?token=abc123 (press Ctrl+C to stop)";
        assert_eq!(
            extract_token_url(line),
            Some("http://127.0.0.1:3080/?token=abc123".into())
        );
    }

    #[test]
    fn extracts_url_with_other_query_params() {
        let line = "dsh web: http://localhost:3080/?foo=bar&token=xyz789";
        assert_eq!(
            extract_token_url(line),
            Some("http://localhost:3080/?foo=bar&token=xyz789".into())
        );
    }
}
