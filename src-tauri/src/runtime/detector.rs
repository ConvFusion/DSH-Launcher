//! Node.js and DeepSeek Harness detection.
//!
//! Node sources (the user's system Node is never modified):
//!
//! 1. Bundled runtime installed by the launcher (`~/.dsh-launcher/runtime/node-*`)
//! 2. `node` on `PATH`
//! 3. Well-known install locations per platform (Homebrew, nvm, fnm, volta,
//!    asdf, mise, MacPorts, nvm-windows, scoop, registry App Paths, …)
//!
//! **All** candidates from **all** sources are collected and the highest
//! version wins. This matters because apps launched from the Finder/Dock
//! get a minimal PATH: the first `node` on PATH (e.g. a stale v18 left in
//! `/usr/local/bin`) must never shadow a newer v22/v24 installed via nvm or
//! Homebrew. Anything older than Node 20 is considered incompatible.

use super::{DshInfo, NodeInfo, NodeSource};
use crate::config::{dsh_dir, log, runtime_dir};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    if let Ok(entries) = fs_read_dir(&runtime_dir()) {
        for entry in entries {
            let dir = entry.join("bin");
            let bin = if cfg!(windows) { dir.join("node.exe") } else { dir.join("node") };
            add(&bin, NodeSource::Bundled);
        }
    }

    // 2. Every `node` on PATH — not just the first one.
    if let Ok(path_var) = std::env::var("PATH") {
        let name = if cfg!(windows) { "node.exe" } else { "node" };
        for dir in std::env::split_paths(&path_var) {
            add(&dir.join(name), NodeSource::System);
        }
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
                "node detection failed: no Node.js found on PATH ({}) or in any known location",
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
            out.push(PathBuf::from(la).join("Programs/nodejs/node.exe"));
            // fnm multishells: one snapshot dir per shell that activated fnm
            if let Ok(entries) = fs::read_dir(PathBuf::from(la).join("fnm_multishells")) {
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
    std::fs::read_dir(p)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_dir())
                .collect()
        })
        .map_err(|e| e)
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

/// A `dsh` binary found on PATH (global npm, npx cache, …). Resolves
/// symlinks so we reach the real package directory.
fn detect_path_dsh() -> Option<DshInfo> {
    let path_var = std::env::var("PATH").ok()?;
    let name = if cfg!(windows) { "dsh.cmd" } else { "dsh" };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if !candidate.is_file() {
            continue;
        }
        // The shim is usually …/node_modules/.bin/dsh → real bin.js, so
        // resolve symlinks first, then walk up to …/@deepseek-ai/dsh.
        let mut p = std::fs::canonicalize(&candidate).unwrap_or(candidate);
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
/// without depending on the `npm`/`npm.cmd` shims on PATH.
pub fn npm_cli_for(node: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(node).unwrap_or_else(|_| node.to_path_buf());
    let home = real.parent()?.parent()?;
    let cli = home.join("lib/node_modules/npm/bin/npm-cli.js");
    cli.exists().then(|| cli)
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




