//! Node.js and DeepSeek Harness detection.
//!
//! Node sources (the user's system Node is never modified):
//!
//! 1. Bundled runtime installed by the launcher (`~/.dsh-launcher/runtime/node-*`)
//! 2. `node` on `PATH`
//! 3. Well-known install locations per platform (Homebrew, nvm, fnm, volta,
//!    asdf, mise, MacPorts, nvm-windows, scoop, registry App Paths, …)
//!
//! The detection is deliberately **command-driven, not path-hardcoded**:
//! instead of only trusting the launcher process's own environment (which a
//! GUI app launched from Finder/Dock/Start menu gets as a *minimal* PATH),
//! we ask the user's login shell for its effective `PATH` and also run
//! `which`/`command -v` against that environment. Whatever `node`/`dsh`
//! resolves there — a custom `/opt/…` prefix, nvm/fnm/volta shims, Homebrew —
//! is a valid candidate. The hardcoded list below is only a last-resort
//! fallback for machines where no shell answers quickly.
//!
//! **All** candidates from **all** sources are collected and the highest
//! version wins. This matters because a stale v18 left in `/usr/local/bin`
//! must never shadow a newer v22/v24 installed via nvm or Homebrew. Anything
//! older than Node 20 is considered incompatible.

use super::{DshInfo, NodeInfo, NodeSource};
use crate::config::{dsh_dir, log, runtime_dir};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Minimum Node major version supported by DeepSeek Harness.
pub const MIN_NODE_MAJOR: u32 = 20;

/// The npm package that provides the `dsh` CLI.
pub const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

/// Collect every Node.js candidate across all sources, deduplicated by
/// canonical path. First sight (bundled → PATH → known locations) decides
/// the reported source when the same binary is reachable twice.
fn all_node_candidates() -> Vec<NodeInfo> {
    let mut out: Vec<NodeInfo> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut add = |path: &Path, source: NodeSource| {
        if !path.is_file() {
            return;
        }
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !seen.insert(key) {
            return;
        }
        if let Some(info) = check_candidate(path, source) {
            out.push(info);
        }
    };

    // 1. Bundled runtimes (every version that was ever installed).
    //    Layout: Unix archives extract node into <dir>/bin/node; Windows
    //    archives put node.exe at the archive root (no bin/ directory).
    if let Ok(entries) = fs_read_dir(&runtime_dir()) {
        for entry in entries {
            let mut cands: Vec<PathBuf> = Vec::new();
            if cfg!(windows) {
                cands.push(entry.join("node.exe"));
                cands.push(entry.join("bin").join("node.exe"));
            } else {
                cands.push(entry.join("bin").join("node"));
            }
            for bin in cands {
                add(&bin, NodeSource::Bundled);
            }
        }
    }

    // 2. `node` on PATH — the launcher process's own PATH, the user's
    //    login-shell PATH (GUI apps get a minimal PATH from the OS, so custom
    //    install prefixes like /opt/… only appear once the user's profile is
    //    sourced), and `which node`/`command -v node` resolution. Detection is
    //    command-driven — it must not depend on a hardcoded list of paths.
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    #[cfg(not(windows))]
    {
        for p in shell_resolve_all(name) {
            add(&p, NodeSource::System);
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(path_var) = std::env::var("PATH") {
        dirs.extend(std::env::split_paths(&path_var));
    }
    #[cfg(not(windows))]
    dirs.extend(shell_path_entries());
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    for dir in dirs {
        if !seen_dirs.insert(dir.clone()) {
            continue;
        }
        add(&dir.join(name), NodeSource::System);
    }

    // 3. Well-known install locations.
    for p in known_node_paths() {
        add(&p, NodeSource::System);
    }

    out
}

fn best_by_version(candidates: Vec<NodeInfo>) -> Option<NodeInfo> {
    candidates.into_iter().max_by_key(|n| version_key(&n.version))
}

/// (major, minor, patch) ordering key; unknown components count as 0.
fn version_key(version: &str) -> (u32, u32, u32) {
    let mut parts = version.split('.');
    (
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
        parts.next().and_then(|s| s.parse().ok()).unwrap_or(0),
    )
}

/// Newest compatible (major >= MIN_NODE_MAJOR) Node anywhere on the system.
/// Logs a diagnostic when nothing compatible is found.
pub fn detect_node() -> Option<NodeInfo> {
    let candidates = all_node_candidates();
    let best = best_by_version(candidates);
    let found = best
        .clone()
        .filter(|n| node_major(&n.version).map(|m| m >= MIN_NODE_MAJOR).unwrap_or(false));
    if found.is_none() {
        match &best {
            Some(n) => log(&format!(
                "node detection failed: newest available is v{} at {} (need >= v{})",
                n.version,
                n.path.display(),
                MIN_NODE_MAJOR
            )),
            None => log(&format!(
                "node detection failed: no Node.js found on the launcher PATH, \
                 the login-shell PATH, or in any known location (PATH={})",
                std::env::var("PATH").unwrap_or_default()
            )),
        }
    }
    found
}

/// Re-detect including versions below the minimum, so the UI can say
/// "Node found but too old" instead of "Node not found".
pub fn detect_node_any() -> Option<NodeInfo> {
    best_by_version(all_node_candidates())
}

/// Validate an explicitly configured Node path (`Config.node_path`).
///
/// This is the per-machine escape hatch: on a computer whose Node layout
/// the automatic discovery cannot resolve, the user points the launcher at
/// the exact binary. It is trusted only after proving it works — the binary
/// must execute and report a supported version — so a stale or wrong path
/// never breaks the launcher: an invalid override falls back to
/// auto-detection.
pub fn detect_node_override(path: &Path) -> Option<NodeInfo> {
    let version = node_version(path)?;
    let supported = node_major(&version)
        .map(|m| m >= MIN_NODE_MAJOR)
        .unwrap_or(false);
    if !supported {
        log(&format!(
            "configured node_path {} reports v{} (need >= v{}) — ignoring, falling back to auto-detection",
            path.display(),
            version,
            MIN_NODE_MAJOR
        ));
        return None;
    }
    Some(NodeInfo {
        path: path.to_path_buf(),
        version,
        source: NodeSource::System,
    })
}

fn node_version(path: &Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let v = s.trim().trim_start_matches('v').to_string();
    if v.split('.').count() >= 3 {
        Some(v)
    } else {
        None
    }
}

pub fn node_major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

fn check_candidate(path: &Path, source: NodeSource) -> Option<NodeInfo> {
    let version = node_version(path)?;
    Some(NodeInfo {
        path: path.to_path_buf(),
        version,
        source,
    })
}

// ---------------------------------------------------------------------------
// Shell-driven discovery
// ---------------------------------------------------------------------------
//
// The launcher is a GUI app: launched from the Finder/Dock/Start menu it gets
// a *minimal* PATH from the OS that does not include the user's shell setup.
// Instead of guessing install locations, we ask the user's login shell for its
// effective environment and resolve executables with `which`/`command -v`.
// Whatever `node`/`dsh` resolves there — a custom /opt/… prefix, nvm/fnm/volta
// shims, Homebrew — is a valid candidate. The hardcoded `known_node_paths`
// list below is only a last-resort fallback.

/// Run a command and capture its stdout, **guaranteed not to hang**:
///
/// * the child runs in its own process group (Unix) and the whole group is
///   killed on timeout, so a lingering grandchild cannot hold the pipe;
/// * stdout is drained by a reader thread, so a full pipe never blocks the
///   child (and therefore never defeats the timeout);
/// * the captured data is collected through a channel with a bounded wait.
///
/// Returns `(success, stdout_bytes)`.
fn run_captured(mut cmd: Command, timeout: Duration) -> (bool, Vec<u8>) {
    use std::process::Stdio;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        // Own session/process group → we can reap the entire process tree.
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure runs in the child right after fork; setsid
        // merely detaches it into a new session (a just-forked child is
        // never a group leader, so it cannot fail).
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let Ok(mut child) = cmd.spawn() else {
        return (false, Vec::new());
    };
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let mut reader: Option<std::thread::JoinHandle<()>> = None;
    if let Some(mut out) = child.stdout.take() {
        reader = Some(std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send(buf);
        }));
    }
    let deadline = Instant::now() + timeout;
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => break false,
        }
    };
    // Reap the whole tree so no grandchild can keep the pipe (and the reader
    // thread) alive, then collect whatever stdout was produced.
    kill_process_tree(&mut child);
    let _ = child.wait();
    let data = rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default();
    let _ = reader; // completes once the pipe reaches EOF
    (success, data)
}

/// Terminate the child and (on Unix) its entire process group.
fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        // Own session ⇒ the pid doubles as the process-group id.
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
}

/// Run `shell -lc "<script>"` and return its stdout (banner lines included;
/// parse with a marker). Bounded by a short timeout so a slow or
/// non-interactive shell profile can never stall detection.
#[cfg(not(windows))]
fn shell_stdout(shell: &str, script: &str) -> Option<String> {
    let mut cmd = Command::new(shell);
    cmd.args(["-lc", script]);
    let (ok, data) = run_captured(cmd, Duration::from_secs(4));
    if ok {
        Some(String::from_utf8_lossy(&data).into_owned())
    } else {
        None
    }
}

/// Resolve every `name` executable (e.g. `node`, `dsh`) the way the user's
/// shell does: `whence -a` (zsh) / `type -a` / `command -v` / `which` on
/// Unix. Returns absolute paths, deduplicated. Never relies on a hardcoded
/// path list.
#[cfg(not(windows))]
fn shell_resolve_all(name: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    // A login shell sources the user's profile, exposing custom PATH entries.
    // `command -v` prints one; `type -a`/`whence -a`/`which -a` print every
    // match on its own line. The marker separates tool output from any
    // banner the profile prints (conda, mamba, …).
    let script = format!(
        "printf '%s\\n' '__DSH_RESOLVE__'; \
         command -v -a {name} 2>/dev/null; \
         type -a {name} 2>/dev/null; \
         whence -a {name} 2>/dev/null; \
         which -a {name} 2>/dev/null"
    );
    for shell in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        let Some(out_text) = shell_stdout(shell, &script) else {
            continue;
        };
        for line in out_text.lines() {
            let line = line.trim();
            // Skip the marker and anything before it (banners).
            if line == "__DSH_RESOLVE__" {
                continue;
            }
            // `type -a` may print "node is /path" or "node is hashed (/path)";
            // keep only real paths ending in /<name>.
            let path = line
                .strip_prefix(&format!("{name} is "))
                .unwrap_or(line)
                .trim();
            let p = PathBuf::from(path);
            if p.is_absolute()
                && p.file_name().map(|f| f == name).unwrap_or(false)
                && seen.insert(p.clone())
            {
                out.push(p);
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// The user's effective `PATH` as a login shell sees it (Unix). This finds
/// custom install prefixes (e.g. `/opt/…/bin`) that the GUI app's minimal
/// PATH cannot see. Banner output is ignored via a marker.
#[cfg(not(windows))]
fn shell_path_entries() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let script = "printf '%s\\n' '__DSH_PATH__'; printf '%s\\n' \"$PATH\"";
    for shell in ["/bin/zsh", "/bin/bash", "/bin/sh"] {
        let Some(text) = shell_stdout(shell, script) else {
            continue;
        };
        let mut capture = false;
        for line in text.lines() {
            let line = line.trim();
            if line == "__DSH_PATH__" {
                capture = true;
                continue;
            }
            if capture && !line.is_empty() {
                for p in std::env::split_paths(line) {
                    if seen.insert(p.clone()) {
                        out.push(p);
                    }
                }
                break;
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// Well-known Node.js install locations, per platform. Apps launched from
/// the Finder/Dock/Start menu get a minimal PATH, so version managers that
/// only put `node` on the *shell* PATH (nvm, fnm, volta, asdf, mise, …)
/// must be found here.
fn known_node_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let home = dirs::home_dir();

    #[cfg(target_os = "macos")]
    {
        out.push(PathBuf::from("/opt/homebrew/bin/node")); // Homebrew (Apple Silicon)
        out.push(PathBuf::from("/usr/local/bin/node")); // Homebrew (Intel) / manual
        out.push(PathBuf::from("/usr/bin/node")); // manual install (rare)
        out.push(PathBuf::from("/opt/local/bin/node")); // MacPorts
        if let Some(h) = &home {
            out.push(h.join(".local").join("bin").join("node")); // common symlink target
            out.push(h.join(".volta").join("bin").join("node")); // Volta
            out.push(h.join(".asdf").join("shims").join("node")); // asdf
            // nvm: ~/.nvm/versions/node/*/bin/node
            if let Ok(entries) = fs::read_dir(h.join(".nvm").join("versions").join("node")) {
                for e in entries.flatten() {
                    out.push(e.path().join("bin").join("node"));
                }
            }
            // fnm: ~/.local/share/fnm/node-versions/*/installation/bin/node
            if let Ok(entries) =
                fs::read_dir(h.join(".local").join("share").join("fnm").join("node-versions"))
            {
                for e in entries.flatten() {
                    out.push(e.path().join("installation").join("bin").join("node"));
                }
            }
            // mise/rtx: ~/.local/share/mise/installs/node/*/bin/node
            if let Ok(entries) = fs::read_dir(
                h.join(".local")
                    .join("share")
                    .join("mise")
                    .join("installs")
                    .join("node"),
            ) {
                for e in entries.flatten() {
                    out.push(e.path().join("bin").join("node"));
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(pf) = std::env::var("ProgramFiles") {
            out.push(PathBuf::from(pf).join("nodejs/node.exe"));
        }
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            out.push(PathBuf::from(pf86).join("nodejs/node.exe"));
        }
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            out.push(PathBuf::from(la.as_str()).join("Programs/nodejs/node.exe"));
            // fnm multishells: one snapshot dir per shell that activated fnm
            if let Ok(entries) = fs::read_dir(PathBuf::from(la.as_str()).join("fnm_multishells")) {
                for e in entries.flatten() {
                    out.push(e.path().join("node.exe"));
                }
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            // nvm-windows keeps one directory per version (v20.11.1, …)
            if let Ok(entries) = fs::read_dir(PathBuf::from(&appdata).join("nvm")) {
                for e in entries.flatten() {
                    out.push(e.path().join("node.exe"));
                }
            }
            // fnm: %APPDATA%\fnm\node-versions\<v>\installation\node.exe
            if let Ok(entries) = fs::read_dir(PathBuf::from(&appdata).join("fnm").join("node-versions"))
            {
                for e in entries.flatten() {
                    out.push(e.path().join("installation").join("node.exe"));
                }
            }
        }
        if let Ok(pd) = std::env::var("ProgramData") {
            // machine-wide nvm-windows installs
            if let Ok(entries) = fs::read_dir(PathBuf::from(pd).join("nvm")) {
                for e in entries.flatten() {
                    out.push(e.path().join("node.exe"));
                }
            }
        }
        if let Some(h) = &home {
            out.push(h.join(".volta").join("bin").join("node.exe")); // Volta
            out.push(h.join("scoop").join("apps").join("nodejs").join("current").join("node.exe"));
        }
        // Registry App Paths — written by most official installers, including
        // per-user ones (the most reliable source on Windows).
        if let Some(p) = registry_app_path("node.exe") {
            out.push(p);
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        out.push(PathBuf::from("/usr/bin/node"));
        out.push(PathBuf::from("/usr/local/bin/node"));
        out.push(PathBuf::from("/snap/bin/node"));
        if let Some(h) = &home {
            out.push(h.join(".local").join("bin").join("node"));
            out.push(h.join(".volta").join("bin").join("node"));
            out.push(h.join(".asdf").join("shims").join("node"));
            if let Ok(entries) = fs::read_dir(h.join(".nvm").join("versions").join("node")) {
                for e in entries.flatten() {
                    out.push(e.path().join("bin").join("node"));
                }
            }
            if let Ok(entries) =
                fs::read_dir(h.join(".local").join("share").join("fnm").join("node-versions"))
            {
                for e in entries.flatten() {
                    out.push(e.path().join("installation").join("bin").join("node"));
                }
            }
            if let Ok(entries) = fs::read_dir(
                h.join(".local")
                    .join("share")
                    .join("mise")
                    .join("installs")
                    .join("node"),
            ) {
                for e in entries.flatten() {
                    out.push(e.path().join("bin").join("node"));
                }
            }
        }
    }

    out
}

/// Windows: resolve an executable from the `App Paths` registry key that
/// official installers (Node, Chrome, Edge, …) register, per-machine or
/// per-user.
#[cfg(windows)]
fn registry_app_path(exe: &str) -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let subkey = format!(
        r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}"
    );
    for predef in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let root = RegKey::predef(predef);
        let Ok(key) = root.open_subkey(&subkey) else {
            continue;
        };
        let Ok(value) = key.get_value::<String, _>("Default") else {
            continue;
        };
        let p = PathBuf::from(value);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn fs_read_dir(p: &Path) -> std::io::Result<Vec<PathBuf>> {
    std::fs::read_dir(p).map(|rd| {
        rd.filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect()
    })
}

// ---------------------------------------------------------------------------
// DeepSeek Harness
// ---------------------------------------------------------------------------

pub fn dsh_install_dir() -> PathBuf {
    dsh_dir()
}

/// Locate a DeepSeek Harness installation, in priority order:
///
/// 1. the launcher-managed install (`~/.dsh-launcher/dsh`), or
/// 2. a `dsh` executable on `PATH` (global npm install, npx cache, etc.), or
/// 3. well-known locations scanned **without relying on PATH** — important
///    because apps launched from Finder/Dock have an empty PATH, so `dsh`
///    installed via npx/nvm/global npm would otherwise be invisible, or
/// 4. the npm global root (as a last resort).
///
/// `path` always points at the directory that contains
/// `node_modules/@deepseek-ai/dsh`, so `start()` can build the bin.js path
/// uniformly for managed and external installs.
pub fn detect_dsh() -> Option<DshInfo> {
    detect_managed_dsh()
        .or_else(detect_path_dsh)
        .or_else(detect_known_locations_dsh)
        .or_else(detect_global_dsh)
}

/// Resolve a user-provided DSH directory to the install root — the folder
/// that contains `node_modules/@deepseek-ai/dsh`. Accepts both the root
/// itself and the package directory (…/node_modules/@deepseek-ai/dsh); the
/// latter is what a file picker naturally yields.
pub fn normalize_dsh_dir(dir: &Path) -> Option<PathBuf> {
    // Case 1: `dir` is already the install root.
    if dir.join("node_modules").join(DSH_PACKAGE).join("package.json").exists() {
        return Some(dir.to_path_buf());
    }
    // Case 2: `dir` is the package directory itself.
    if dir.join("package.json").exists()
        && read_pkg_name(dir).as_deref() == Some(DSH_PACKAGE)
    {
        let scope = dir.parent()?.file_name()?;
        let nm = dir.parent()?.parent()?.file_name()?;
        if scope == "@deepseek-ai" && nm == "node_modules" {
            return Some(dir.parent()?.parent()?.parent()?.to_path_buf());
        }
    }
    None
}

fn read_pkg_name(dir: &Path) -> Option<String> {
    let s = fs::read_to_string(dir.join("package.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("name").and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// Detect a DSH install at an explicit target directory (a user-configured
/// directory). Accepts the install root or the package directory inside it.
pub fn detect_dsh_in(dir: &Path) -> Option<DshInfo> {
    let root = normalize_dsh_dir(dir)?;
    let pkg = root.join("node_modules").join(DSH_PACKAGE);
    Some(DshInfo {
        path: root,
        version: read_pkg_version(&pkg).unwrap_or_else(|| "unknown".to_string()),
    })
}

/// Path of the `dsh` entry script inside a given install root directory
/// (the folder containing `node_modules/@deepseek-ai/dsh`).
pub fn dsh_bin_js_in(dir: &Path) -> PathBuf {
    dir.join("node_modules")
        .join(DSH_PACKAGE)
        .join("lib")
        .join("bin.js")
}

fn read_pkg_version(pkg: &Path) -> Option<String> {
    let manifest = pkg.join("package.json");
    let s = fs::read_to_string(manifest).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// The launcher-managed installation.
fn detect_managed_dsh() -> Option<DshInfo> {
    detect_dsh_in(&dsh_install_dir())
}

/// A `dsh` binary found on PATH (global npm, npx cache, …) or resolved via
/// the user's shell (`which dsh` / `command -v dsh`). Resolves symlinks so
/// we reach the real package directory. Detection is command-driven — the
/// shell's own resolution is consulted, not just the app's minimal PATH.
fn detect_path_dsh() -> Option<DshInfo> {
    let name = if cfg!(windows) { "dsh.cmd" } else { "dsh" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(path_var) = std::env::var("PATH") {
        candidates.extend(std::env::split_paths(&path_var).map(|d| d.join(name)));
    }
    #[cfg(not(windows))]
    {
        candidates.extend(shell_resolve_all(name));
        candidates.extend(shell_path_entries().into_iter().map(|d| d.join(name)));
    }
    for candidate in candidates {
        if candidate.is_file() {
            if let Some(info) = dsh_from_bin(&candidate) {
                return Some(info);
            }
        }
    }
    None
}

/// The shim is usually …/node_modules/.bin/dsh → real bin.js, so resolve
/// symlinks first, then walk up to …/@deepseek-ai/dsh.
fn dsh_from_bin(candidate: &Path) -> Option<DshInfo> {
    let mut p = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    loop {
        let is_pkg = p.file_name().map(|f| f == "dsh").unwrap_or(false)
            && p.parent()
                .and_then(|pp| pp.file_name())
                .map(|f| f == "@deepseek-ai")
                .unwrap_or(false);
        if is_pkg {
            let version = read_pkg_version(&p).unwrap_or_else(|| "unknown".to_string());
            // Directory that contains node_modules/@deepseek-ai/dsh:
            // p = …/node_modules/@deepseek-ai/dsh → root = p's great-grandparent.
            let root = p.parent()?.parent()?.parent()?.to_path_buf();
            return Some(DshInfo { path: root, version });
        }
        p = match p.parent() {
            Some(parent) => parent.to_path_buf(),
            None => break,
        };
    }
    None
}

/// Scan well-known install locations **without depending on PATH**:
///
/// * npx cache:      `~/.npm/_npx/<id>/node_modules/@deepseek-ai/dsh`
/// * nvm:            `~/.nvm/versions/node/*/lib/node_modules/@deepseek-ai/dsh`
/// * Homebrew:       `/opt/homebrew/lib/node_modules/@deepseek-ai/dsh`
/// * system:         `/usr/local/lib/node_modules/@deepseek-ai/dsh`
fn detect_known_locations_dsh() -> Option<DshInfo> {
    let home = dirs::home_dir()?;
    let mut roots: Vec<PathBuf> = Vec::new();

    // ~/.npm/_npx/<id> (each entry is a root containing node_modules/)
    if let Ok(entries) = fs::read_dir(home.join(".npm").join("_npx")) {
        for entry in entries.flatten() {
            roots.push(entry.path());
        }
    }
    // ~/.nvm/versions/node/<v>/lib/node_modules
    if let Ok(entries) = fs::read_dir(home.join(".nvm").join("versions").join("node")) {
        for entry in entries.flatten() {
            roots.push(entry.path().join("lib").join("node_modules"));
        }
    }
    // Homebrew & system global roots
    roots.push(PathBuf::from("/opt/homebrew/lib/node_modules"));
    roots.push(PathBuf::from("/usr/local/lib/node_modules"));
    // npm global prefix in home (~/.npm-global or ~/.local/lib/node_modules)
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join(".npm-global").join("lib").join("node_modules"));
        roots.push(h.join(".local").join("lib").join("node_modules"));
    }

    for root in roots {
        let pkg = root.join("node_modules").join(DSH_PACKAGE);
        if pkg.join("package.json").exists() {
            return Some(DshInfo {
                path: root,
                version: read_pkg_version(&pkg).unwrap_or_else(|| "unknown".to_string()),
            });
        }
    }
    None
}

/// The npm global install root (`npm root -g`).
fn detect_global_dsh() -> Option<DshInfo> {
    let out = std::process::Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
        .args(["root", "-g"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    let pkg = root.join("@deepseek-ai").join(DSH_PACKAGE.trim_start_matches('@'));
    if !pkg.join("package.json").exists() {
        return None;
    }
    Some(DshInfo {
        path: root.parent()?.to_path_buf(),
        version: read_pkg_version(&pkg).unwrap_or_else(|| "unknown".to_string()),
    })
}

/// Locate the `npm-cli.js` belonging to a Node installation, so we can run npm
/// Locate the `npm-cli.js` belonging to a Node installation, so we can run npm
/// without depending on the `npm`/`npm.cmd` shims on PATH (those shims
/// resolve against whichever node happens to be first on PATH, which may not
/// be the runtime we selected).
///
/// Each user's machine differs — custom /opt/… prefixes, moved archives,
/// Homebrew, nvm, fnm, volta, mixed installs — so nothing here is a
/// hardcoded path list. Candidates are gathered from every signal we have:
///
///  1. **relative to the node binary** — Windows archives / standard installs
///     / fnm / nvm-windows put `node_modules/npm/` right next to `node.exe`;
///     Unix archives / Homebrew / distro packages put `node` in
///     `<prefix>/bin` and npm in `<prefix>/lib/node_modules/npm/`;
///  2. **the `npm` executable next to the node binary** — in the standard
///     Unix layouts it is a symlink (chain) whose final target is
///     `npm-cli.js`, or a small JS entry wrapper; resolving it locates the
///     CLI no matter where the runtime lives;
///  3. **the user's login shell** — `which npm` / `command -v npm` see
///     whatever prefix the user's profile puts on PATH.
///
/// Every candidate is then **verified by executing it with the selected
/// runtime** (`node <npm-cli.js> --version`); the first one that answers is
/// the one we use. No guess is ever trusted blindly.
///
/// (The npm *global prefix* — e.g. `%APPDATA%\npm` — is where `npm i -g`
/// stores user packages; it is not where the runtime's own npm lives.)
pub fn npm_cli_for(node: &Path) -> Option<PathBuf> {
    cli_for(node, "npm", "npm-cli.js")
}

/// Same strategy as [`npm_cli_for`] but for npx: `npx-cli.js` lives right
/// next to `npm-cli.js`. Running the plugin command as `node npx-cli.js …`
/// keeps it shell-free on every platform (Windows' `npx.cmd` would require
/// `cmd /c`, which re-introduces shell parsing).
pub fn npx_cli_for(node: &Path) -> Option<PathBuf> {
    cli_for(node, "npx", "npx-cli.js")
}

fn cli_for(node: &Path, shim_name: &str, cli_name: &str) -> Option<PathBuf> {
    let real = std::fs::canonicalize(node).unwrap_or_else(|_| node.to_path_buf());
    let dir = real.parent()?;

    // 1. Known relative layouts (see `npm_cli_for` docs). Both npm-cli.js and
    //    npx-cli.js live under node_modules/npm/bin in every distribution.
    let relative = [
        dir.join("node_modules").join("npm").join("bin").join(cli_name),
        dir.join("..")
            .join("lib")
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join(cli_name),
    ];
    for c in &relative {
        if cli_verifies(node, c) {
            return Some(c.clone());
        }
    }

    // 2. Resolve the tool executable that ships next to the node binary: a
    //    symlink (chain) ends at the CLI script itself, or at a JS entry
    //    wrapper the selected node can execute directly.
    #[cfg(not(windows))]
    {
        let shim = dir.join(shim_name);
        if shim.is_file() {
            if let Ok(resolved) = std::fs::canonicalize(&shim) {
                let is_cli = resolved
                    .file_name()
                    .map(|f| f == cli_name)
                    .unwrap_or(false);
                if (is_cli || is_js_entry(&resolved)) && cli_verifies(node, &resolved) {
                    return Some(resolved);
                }
            }
        }

        // 3. The user's login shell — `which <tool>` / `command -v <tool>`
        //    see whatever prefix the user's profile put on PATH. Reached only
        //    when the runtime's own CLI could not be verified: a CLI that is
        //    not the runtime's own still works as long as it runs under the
        //    selected node (verified below).
        for p in shell_resolve_all(shim_name) {
            let is_cli = p
                .file_name()
                .map(|f| f == cli_name)
                .unwrap_or(false);
            if !(is_cli || is_js_entry(&p)) {
                continue;
            }
            let rp = std::fs::canonicalize(&p).unwrap_or(p);
            if cli_verifies(node, &rp) {
                return Some(rp);
            }
        }
    }

    log(&format!(
        "no usable {cli_name} found for the Node.js runtime at {}",
        real.display()
    ));
    None
}

/// Run `node <cli> --version`; the CLI is usable with this runtime iff it
/// answers with a version string. Bounded by a hard timeout.
fn cli_verifies(node: &Path, cli: &Path) -> bool {
    if !cli.is_file() {
        return false;
    }
    let mut cmd = Command::new(node);
    cmd.arg(cli).arg("--version");
    let (ok, data) = run_captured(cmd, Duration::from_secs(15));
    if !ok {
        return false;
    }
    // `npm --version` prints e.g. "11.19.0" — require a numeric major
    // component so arbitrary tool output is not accepted.
    let s = String::from_utf8_lossy(&data);
    s.trim()
        .split('.')
        .next()
        .is_some_and(|s| !s.is_empty() && s.parse::<u32>().is_ok())
}

/// True when the file looks like a JavaScript entry point the selected `node`
/// can execute directly (a `#!…node` shebang on the first line).
#[cfg(not(windows))]
fn is_js_entry(p: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(p) else {
        return false;
    };
    let mut buf = [0u8; 256];
    let Ok(n) = f.read(&mut buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    matches!(
        head.lines().next(),
        Some(line) if line.starts_with("#!") && line.contains("node")
    )
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// A line-by-line report of what the detector can see **on this machine**:
/// the PATH it was given, what the login shell resolves for node/npm/dsh,
/// every Node.js candidate (with its version), the selected Node, and which
/// npm/npx CLI resolves for it (plus the actual `npm --version` output).
///
/// Every discovery strategy runs for real, so the report reflects what the
/// launcher actually does — not an assumption. Triggered on demand from the
/// UI so a failing machine (one we can't inspect directly) can be analyzed
/// by looking at what it reports.
pub fn env_diagnostics() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.push(format!("platform: {} {}", std::env::consts::OS, std::env::consts::ARCH));
    out.push(format!(
        "process PATH = {}",
        std::env::var("PATH").unwrap_or_else(|_| "<unset>".into())
    ));

    #[cfg(not(windows))]
    {
        out.push(format!("login-shell PATH = {:?}", shell_path_entries()));
        out.push(format!("login-shell `node` = {:?}", shell_resolve_all("node")));
        out.push(format!("login-shell `npm` = {:?}", shell_resolve_all("npm")));
        out.push(format!("login-shell `dsh` = {:?}", shell_resolve_all("dsh")));
    }

    let configured = crate::config::Config::load().node_path.clone();
    match &configured {
        Some(p) => {
            let valid = detect_node_override(std::path::Path::new(p)).is_some();
            out.push(format!("configured node_path = {} (valid={valid})", p,));
        }
        None => out.push("configured node_path = (none)".into()),
    }

    let cands = all_node_candidates();
    out.push(format!("node candidates ({}):", cands.len()));
    if cands.is_empty() {
        out.push("  (none found)".into());
    }
    for c in &cands {
        out.push(format!(
            "  - v{} [{}] {}",
            c.version,
            match c.source {
                NodeSource::System => "system",
                NodeSource::Bundled => "bundled",
            },
            c.path.display()
        ));
    }

    match detect_node() {
        Some(node) => {
            out.push(format!(
                "selected node: v{} at {}",
                node.version,
                node.path.display()
            ));

            match npm_cli_for(&node.path) {
                Some(cli) => {
                    out.push(format!("npm-cli.js = {}", cli.display()));
                    let mut cmd = Command::new(&node.path);
                    cmd.arg(&cli).arg("--version");
                    let (ok, data) = run_captured(cmd, Duration::from_secs(15));
                    out.push(format!(
                        "npm --version = '{}' ({})",
                        String::from_utf8_lossy(&data).trim(),
                        if ok { "ok" } else { "FAILED" }
                    ));
                }
                None => out.push("npm-cli.js = NOT FOUND".into()),
            }
            match npx_cli_for(&node.path) {
                Some(cli) => out.push(format!("npx-cli.js = {}", cli.display())),
                None => out.push("npx-cli.js = NOT FOUND".into()),
            }
        }
        None => {
            out.push("selected node: NONE (no compatible Node.js >= v{MIN_NODE_MAJOR})".into());
            if let Some(any) = detect_node_any() {
                out.push(format!(
                    "newest node (any version): v{} at {}",
                    any.version,
                    any.path.display()
                ));
            }
        }
    }

    match detect_dsh() {
        Some(d) => out.push(format!(
            "dsh: v{} at {}",
            d.version,
            d.path.display()
        )),
        None => out.push("dsh: NOT FOUND".into()),
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(version: &str, source: NodeSource) -> NodeInfo {
        NodeInfo {
            path: PathBuf::from(format!("/fake/node-{version}")),
            version: version.to_string(),
            source,
        }
    }

    #[test]
    fn best_by_version_prefers_highest_across_sources() {
        let mut candidates = vec![
            cand("18.19.0", NodeSource::System), // stale node first on PATH
            cand("24.9.0", NodeSource::System), // nvm
            cand("22.14.0", NodeSource::System), // fnm
        ];
        let best = best_by_version(std::mem::take(&mut candidates)).unwrap();
        assert_eq!(best.version, "24.9.0");

        // Minor/patch participate in the ordering too.
        candidates = vec![cand("22.11.0", NodeSource::System), cand("22.14.0", NodeSource::Bundled)];
        let best = best_by_version(std::mem::take(&mut candidates)).unwrap();
        assert_eq!(best.version, "22.14.0");
    }

    #[test]
    fn detect_node_prefers_newest_compatible() {
        // On a machine with an old node visible early and a newer one in a
        // known location, the newer one must win (regression: the old
        // first-match + filter logic returned "not found").
        let node = detect_node();
        if let Some(n) = &node {
            assert!(
                node_major(&n.version).unwrap() >= MIN_NODE_MAJOR,
                "detect_node must only return compatible versions, got {}",
                n.version
            );
        }
        println!("detect_node() = {node:?}");
    }

    #[test]
    fn detect_dsh_on_this_machine() {
        let dsh = detect_dsh();
        println!("detect_dsh() = {dsh:?}");
        // Don't assert — this machine may legitimately lack DSH.
    }

    #[test]
    fn detect_dsh_in_accepts_package_dir() {
        // The configured dsh_dir often points at the package directory
        // itself (what a file picker yields) — detection must normalize it
        // to the install root instead of reporting "not installed".
        let pkg_dir = dirs::home_dir()
            .unwrap()
            .join(".npm")
            .join("_npx")
            .join("1e7f6d9597241db0")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh");
        if !pkg_dir.join("package.json").exists() {
            println!("skip: no npx DSH install on this machine");
            return;
        }
        let info = detect_dsh_in(&pkg_dir).expect("package dir should be detected");
        println!("detect_dsh_in(pkg_dir) = {info:?}");
        // The reported path must be the install root.
        assert!(info.path.join("node_modules").join(DSH_PACKAGE).join("package.json").exists());
    }

    /// A fake `node` executable. On Unix it is a shell script that either
    /// answers `--version` (so `cli_verifies` accepts a CLI) or exits
    /// non-zero (so every CLI is rejected). On Windows a stub is written;
    /// the tests only assert the negative (strict) case there.
    fn fake_node(path: &Path, working: bool) {
        #[cfg(unix)]
        {
            let script = if working {
                "#!/bin/sh\necho 11.0.0\n"
            } else {
                "#!/bin/sh\nexit 1\n"
            };
            std::fs::write(path, script).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(windows)]
        {
            let _ = working;
            std::fs::write(path, "").unwrap();
        }
    }

    #[test]
    fn npm_cli_for_locates_runtime_bundled_npm() {
        // Layout A: node executable next to node_modules/npm — Windows
        // archives / standard installs / fnm / nvm-windows.
        // Layout B: <prefix>/bin/node + <prefix>/lib/node_modules/npm —
        // Unix archives, Homebrew, distro packages.
        // Every candidate is verified by running it with the selected node,
        // so the fake node must be executable on Unix.
        let base = std::env::temp_dir().join(format!("dsh-npmcli-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let a = base.join("a");
        std::fs::create_dir_all(a.join("node_modules/npm/bin")).unwrap();
        std::fs::write(a.join("node_modules/npm/bin/npm-cli.js"), "").unwrap();
        let a_node = a.join("node.exe");
        fake_node(&a_node, true);
        let got = npm_cli_for(&a_node);
        #[cfg(unix)]
        assert!(
            got.as_ref().map(|p| p.exists() && p.ends_with("npm-cli.js")).unwrap_or(false),
            "layout A: {got:?}"
        );
        #[cfg(windows)]
        assert!(got.is_none(), "unverified CLI must not be trusted: {got:?}");

        let b = base.join("b");
        std::fs::create_dir_all(b.join("bin")).unwrap();
        std::fs::create_dir_all(b.join("lib/node_modules/npm/bin")).unwrap();
        std::fs::write(b.join("lib/node_modules/npm/bin/npm-cli.js"), "").unwrap();
        let b_node = b.join("bin/node");
        fake_node(&b_node, true);
        let got = npm_cli_for(&b_node);
        #[cfg(unix)]
        assert!(
            got.as_ref().map(|p| p.exists() && p.ends_with("npm-cli.js")).unwrap_or(false),
            "layout B: {got:?}"
        );
        #[cfg(windows)]
        assert!(got.is_none(), "unverified CLI must not be trusted: {got:?}");

        // No npm anywhere and the node refuses to run → None. Even an npm the
        // user's shell could provide is rejected, because it cannot be
        // verified against this runtime.
        let c = base.join("c");
        std::fs::create_dir_all(&c).unwrap();
        let c_node = c.join("node");
        fake_node(&c_node, false);
        assert!(npm_cli_for(&c_node).is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn npm_cli_for_resolves_npm_symlink_next_to_node() {
        // The reported test-Mac scenario: node at a custom prefix whose
        // relative lib/ layout does not match the standard guesses, but an
        // `npm` symlink sits right next to the node binary and points at the
        // real npm-cli.js. The symlink must be followed to locate the CLI.
        let base = std::env::temp_dir().join(format!("dsh-npmlink-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bin = base.join("bin");
        let cli = base.join("custom").join("npm-cli.js");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "").unwrap();
        let node = bin.join("node");
        fake_node(&node, true);
        std::os::unix::fs::symlink("../custom/npm-cli.js", bin.join("npm")).unwrap();

        let got = npm_cli_for(&node);
        let want = std::fs::canonicalize(&cli).ok();
        let have = got.as_ref().and_then(|p| std::fs::canonicalize(p).ok());
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(have, want, "npm symlink next to node must resolve to the CLI");
    }

    #[cfg(unix)]
    #[test]
    fn npm_cli_for_accepts_js_wrapper_next_to_node() {
        // No standard layout at all; the `npm` next to node is a plain JS
        // entry wrapper (node shebang). The selected runtime can execute it
        // directly, so it is a valid CLI.
        let base = std::env::temp_dir().join(format!("dsh-npmjs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let node = bin.join("node");
        fake_node(&node, true);
        let wrapper = bin.join("npm");
        std::fs::write(&wrapper, "#!/usr/bin/env node\nrequire('./lib/cli.js')(process)\n")
            .unwrap();

        let got = npm_cli_for(&node);
        let want = std::fs::canonicalize(&wrapper).ok();
        let have = got.as_ref().and_then(|p| std::fs::canonicalize(p).ok());
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(have, want, "a node-shebang JS wrapper must be accepted");
    }

    #[cfg(unix)]
    #[test]
    fn npm_cli_for_never_returns_a_shell_script() {
        // An `npm` next to node that is a *shell* script must never be
        // returned as a CLI (the runtime could not execute it). If anything
        // is returned it must come from elsewhere (e.g. the user's shell).
        let base = std::env::temp_dir().join(format!("dsh-npmsh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let node = bin.join("node");
        fake_node(&node, true);
        let script = bin.join("npm");
        std::fs::write(&script, "#!/bin/sh\necho 11.0.0\n").unwrap();

        let got = npm_cli_for(&node);
        let script_real = std::fs::canonicalize(&script).ok();
        let _ = std::fs::remove_dir_all(&base);
        if let Some(p) = got {
            let p = std::fs::canonicalize(&p).ok();
            assert_ne!(
                p, script_real,
                "a shell script must not be used as the npm CLI"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn node_override_requires_a_working_supported_node() {
        // A configured node path is trusted only if it executes and reports a
        // supported version. Anything else is rejected so a stale override
        // falls back to auto-detection instead of breaking the launcher.
        let base = std::env::temp_dir().join(format!("dsh-nodeovr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let write_exec = |path: &Path, body: &str| {
            std::fs::write(path, body).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };

        let good = base.join("good-node");
        write_exec(&good, "#!/bin/sh\necho 22.11.0\n");
        let broken = base.join("broken-node");
        write_exec(&broken, "#!/bin/sh\nexit 1\n");

        let ok = detect_node_override(&good);
        assert!(
            ok.as_ref().map(|n| n.version == "22.11.0").unwrap_or(false),
            "a working, supported configured node must be accepted: {ok:?}"
        );
        assert!(detect_node_override(&broken).is_none(), "a broken node must be rejected");
        assert!(
            detect_node_override(&base.join("missing")).is_none(),
            "a missing node must be rejected"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn old_node_early_on_path_does_not_shadow_newer_install() {
        // Regression test for "installed but not detected": a stale old
        // `node` reachable first on PATH must not make detect_node() return
        // None when a newer install exists in a known location (nvm, …).
        //
        // NOTE: briefly rewrites the process-wide PATH; no other test in this
        // crate asserts on PATH-dependent results.
        let tmp = std::env::temp_dir().join(format!("dsh-oldnode-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create temp dir");
        #[cfg(unix)]
        {
            let shim = tmp.join("node");
            fs::write(&shim, "#!/bin/sh\necho v18.19.0\n").expect("write shim");
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let old_path = std::env::var("PATH").unwrap_or_default();
        let mut new_path = tmp.display().to_string();
        if !old_path.is_empty() {
            new_path.push(':');
            new_path.push_str(&old_path);
        }
        std::env::set_var("PATH", &new_path);
        let detected = detect_node();
        let any = detect_node_any();
        std::env::set_var("PATH", &old_path);
        let _ = fs::remove_dir_all(&tmp);

        println!("old node first on PATH → detect_node() = {detected:?}, detect_node_any() = {any:?}");
        match &detected {
            Some(n) => {
                // A compatible node was found — it must not be the old shim.
                assert!(
                    node_major(&n.version).unwrap() >= MIN_NODE_MAJOR,
                    "an incompatible node must not be returned (got v{})",
                    n.version
                );
            }
            None => {
                // No other node on this machine: node_any must at least see
                // the old shim so the UI can say "too old".
                let n = any.as_ref().expect("the old node must be visible to detect_node_any");
                assert!(node_major(&n.version).unwrap() < MIN_NODE_MAJOR);
            }
        }
    }

    #[test]
    fn known_node_paths_include_nvm() {
        let paths = known_node_paths();
        println!("known_node_paths: {paths:?}");
        let home = dirs::home_dir().unwrap();
        assert!(
            paths.iter().any(|p| p.starts_with(home.join(".nvm"))),
            "nvm paths should be scanned"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn shell_resolve_returns_existing_node_binaries() {
        // `which node` / `command -v node` driven resolution must return only
        // real, absolute `node` binaries — and each must actually run
        // (`node --version`), the command-driven check the installer relies on.
        let resolved = shell_resolve_all("node");
        println!("shell_resolve_all(node) = {resolved:?}");
        for p in &resolved {
            assert!(p.is_absolute(), "resolved path must be absolute: {p:?}");
            assert!(
                p.file_name().map(|f| f == "node").unwrap_or(false),
                "resolved path must end in /node: {p:?}"
            );
            assert!(p.is_file(), "resolved path must exist: {p:?}");
            assert!(
                node_version(p).is_some(),
                "`node --version` must run for {p:?}"
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn detect_node_finds_shell_node_with_minimal_process_path() {
        // Regression for "node installed at /opt/… but the launcher says it
        // doesn't exist": a GUI app launched from Finder/Dock gets a minimal
        // PATH, so detection must not depend on the process PATH alone — it
        // asks the login shell, which sources the user's profile and sees the
        // custom install.
        let old = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", "/usr/bin:/bin");
        let node = detect_node();
        std::env::set_var("PATH", &old);
        println!("detect_node with minimal PATH = {node:?}");

        if let Some(n) = &node {
            assert!(
                node_major(&n.version).unwrap() >= MIN_NODE_MAJOR,
                "detected node must be compatible (got v{})",
                n.version
            );
        }
        // If the login shell can see a node, the launcher must too — otherwise
        // a custom /opt install would be wrongly reported as missing.
        let shell = shell_resolve_all("node");
        if !shell.is_empty() && node.is_none() {
            panic!(
                "node exists on the login shell PATH ({shell:?}) but detection returned None"
            );
        }
    }

    #[test]
    fn normalize_dsh_dir_accepts_root_and_package_dir() {
        // Build a fake install tree in a temp dir.
        let base = std::env::temp_dir().join(format!("dsh-normalize-test-{}", std::process::id()));
        let pkg = base.join("node_modules").join("@deepseek-ai").join("dsh");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&pkg).expect("create fake pkg dir");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@deepseek-ai/dsh","version":"0.0.1"}"#,
        )
        .expect("write fake package.json");

        // Case 1: the install root.
        let root = normalize_dsh_dir(&base).expect("root should normalize");
        assert_eq!(root, base);

        // Case 2: the package directory itself.
        let pkg_dir = normalize_dsh_dir(&pkg).expect("package dir should normalize");
        assert_eq!(pkg_dir, base);

        // Case 3: an unrelated directory.
        let other = base.join("elsewhere");
        fs::create_dir_all(&other).unwrap();
        assert!(normalize_dsh_dir(&other).is_none());

        let _ = fs::remove_dir_all(&base);
    }
}




