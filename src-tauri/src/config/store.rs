//! Configuration store.
//!
//! All launcher state lives under the user data directory:
//!
//! ```text
//! ~/.dsh-launcher/
//! ├── config.json      user preferences
//! ├── state.json       last managed DSH process (pid / port)
//! ├── logs/
//! │   ├── launcher.log
//! │   └── harness.log
//! ├── runtime/         bundled Node.js (installed on demand)
//! └── dsh/             managed DeepSeek Harness installation
//! ```

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;
use time::macros::format_description;
use time::OffsetDateTime;

/// Directory that holds every launcher-owned file.
pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dsh-launcher")
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn runtime_dir() -> PathBuf {
    data_dir().join("runtime")
}

pub fn dsh_dir() -> PathBuf {
    data_dir().join("dsh")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

fn ensure_dirs() {
    let _ = fs::create_dir_all(logs_dir());
}

/// Append a line to the launcher's own log (and stderr, for `tauri dev`).
pub fn log(msg: &str) {
    eprintln!("[dsh-launcher] {msg}");
    let Ok(when) = OffsetDateTime::now_local() else {
        return;
    };
    let ts = when
        .format(format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"))
        .unwrap_or_else(|_| "unknown".to_string());
    ensure_dirs();
    let path = logs_dir().join("launcher.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{ts} {msg}");
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    pub remember: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            r#type: None,
            remember: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3080,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub browser: BrowserConfig,
    pub server: ServerConfig,
    /// "Launch DSH Launcher at system startup"
    pub autostart: bool,
    /// Open the browser after the harness becomes ready on a manual start.
    pub open_browser_on_start: bool,
    /// UI language: "en" (default) or "zh".
    pub language: String,
    /// UI theme: "system" (default), "light" or "dark".
    pub theme: String,
    /// Optional custom directory where DeepSeek Harness is installed /
    /// will be installed. When set, detection and updates target this
    /// directory instead of the default `~/.dsh-launcher/dsh`.
    pub dsh_dir: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            browser: BrowserConfig::default(),
            server: ServerConfig::default(),
            autostart: false,
            open_browser_on_start: true,
            language: "en".to_string(),
            theme: "system".to_string(),
            dsh_dir: None,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        match fs::read_to_string(config_path()) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log(&format!("config.json unreadable ({e}), using defaults"));
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Atomic write: tmp file + rename so a crash never corrupts the config.
    pub fn save(&self) -> Result<(), String> {
        ensure_dirs();
        let json =
            serde_json::to_string_pretty(self).map_err(|e| format!("serialize config: {e}"))?;
        let tmp = data_dir().join("config.json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("write config: {e}"))?;
        fs::rename(&tmp, config_path()).map_err(|e| format!("replace config: {e}"))?;
        log("config saved");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Process state (used for "adopt a previously started instance")
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessStateFile {
    pub pid: u32,
    pub host: String,
    pub port: u16,
    pub started_at: String,
}

pub fn read_process_state() -> Option<ProcessStateFile> {
    let s = fs::read_to_string(state_path()).ok()?;
    serde_json::from_str(&s).ok()
}

pub fn write_process_state(st: &ProcessStateFile) {
    ensure_dirs();
    if let Ok(json) = serde_json::to_string_pretty(st) {
        if let Err(e) = fs::write(state_path(), json) {
            log(&format!("failed to write state.json: {e}"));
        }
    }
}

pub fn clear_process_state() {
    let _ = fs::remove_file(state_path());
}

// ---------------------------------------------------------------------------
// Rolling harness log
// ---------------------------------------------------------------------------

const HARNESS_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

fn harness_log_path() -> PathBuf {
    logs_dir().join("harness.log")
}

/// Global append-only writer for the DSH process output.
static HARNESS_LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn harness_log_line(line: &str) {
    ensure_dirs();
    let path = harness_log_path();
    let mut guard = match HARNESS_LOG.lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    if guard.is_none() || needs_rotation(&path) {
        // Rotate once (keep a single .old copy).
        if guard.is_some() {
            let _ = fs::remove_file(path.with_extension("log.old"));
            let _ = fs::rename(&path, path.with_extension("log.old"));
        }
        match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(f) => *guard = Some(f),
            Err(e) => {
                log(&format!("failed to open harness.log: {e}"));
                return;
            }
        }
    }
    if let Some(f) = guard.as_mut() {
        let ts = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{ts}] {line}");
    }
}

fn needs_rotation(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(m) => m.len() > HARNESS_LOG_MAX_BYTES,
        Err(_) => false,
    }
}

/// Read the last `lines` lines from a log file (capped at 1 MiB scanned).
pub fn read_log_tail(name: &str, lines: usize) -> Result<String, String> {
    let path = match name {
        "harness" => harness_log_path(),
        "launcher" => logs_dir().join("launcher.log"),
        other => return Err(format!("unknown log: {other}")),
    };
    if !path.exists() {
        return Ok(String::new());
    }
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    let len = meta.len();
    let start = len.saturating_sub(1024 * 1024);
    let mut buf = vec![0u8; len.saturating_sub(start) as usize];
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(&path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    f.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let all: Vec<&str> = text.lines().collect();
    if all.len() <= lines {
        // Drop the (possibly partial) first line when we truncated.
        if start > 0 && all.len() > 1 {
            Ok(all[1..].join("\n"))
        } else {
            Ok(text)
        }
    } else {
        Ok(all[all.len() - lines..].join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Small time helper (shared)
// ---------------------------------------------------------------------------

pub fn now_stamp() -> String {
    OffsetDateTime::now_local()
        .map(|t| {
            t.format(format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]"
            ))
            .unwrap_or_else(|_| "unknown".into())
        })
        .unwrap_or_else(|_| "unknown".into())
}



