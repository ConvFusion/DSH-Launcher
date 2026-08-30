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
use crate::runtime::{DshInfo, NodeInfo};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub const READY_TIMEOUT: Duration = Duration::from_secs(120);
const TAIL_LINES: usize = 300;

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
            error: None,
            error_details: None,
            started_at: None,
            tail: VecDeque::new(),
            stop_requested: false,
            external: false,
        }
    }

    fn snapshot(&self) -> ProcSnapshot {
        ProcSnapshot {
            state: self.state,
            pid: self.pid,
            host: self.host.clone(),
            port: self.port,
            url: format!("http://{}:{}", self.host, self.port),
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

        // Mark starting.
        {
            let mut g = self.inner.lock().unwrap();
            g.state = ProcessState::Starting;
            g.error = None;
            g.error_details = None;
            g.tail.clear();
            g.stop_requested = false;
            g.started_at = Some(now_stamp());
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
            use tokio::process::CommandExt;
            // Detach from any console; the child is a GUI service.
            cmd.creation_flags(0x00000200 | 0x00000010); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
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
        });

        // Output pump: both pipes → harness.log + in-memory tail.
        let inner = Arc::clone(&self.inner);
        if let Some(out) = stdout {
            tokio::spawn(reader_task(out, inner.clone()));
        }
        if let Some(err) = stderr {
            tokio::spawn(reader_task_err(err, inner));
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
                "harness process exited (code {exit_code:?}, stop_requested ignored)"
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

async fn reader_task(pipe: tokio::process::ChildStdout, inner: Arc<Mutex<Inner>>) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        config::harness_log_line(&line);
        let mut g = inner.lock().unwrap();
        g.tail.push_back(line.clone());
        while g.tail.len() > TAIL_LINES {
            g.tail.pop_front();
        }
    }
}

async fn reader_task_err(pipe: tokio::process::ChildStderr, inner: Arc<Mutex<Inner>>) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        config::harness_log_line(&line);
        let mut g = inner.lock().unwrap();
        g.tail.push_back(line.clone());
        while g.tail.len() > TAIL_LINES {
            g.tail.pop_front();
        }
    }
}
