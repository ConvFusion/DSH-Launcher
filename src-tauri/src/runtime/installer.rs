//! Installing missing pieces:
//!
//! * Node.js — downloaded from nodejs.org (official, SHA-256 verified) into
//!   `~/.dsh-launcher/runtime/`. The user's system Node is never touched.
//! * DeepSeek Harness — `npm install @deepseek-ai/dsh` into `~/.dsh-launcher/dsh`.

use super::detector::{dsh_bin_js_in, detect_dsh_in, npm_cli_for, DSH_PACKAGE};
use crate::config::{log, runtime_dir};
use std::path::PathBuf;
use std::time::Duration;

/// Fallback Node version used when nodejs.org cannot be reached for its index.
pub const NODE_FALLBACK_VERSION: &str = "v22.14.0";

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent("dsh-launcher/0.1 (+https://github.com/deepseek-ai/deepseek-harness)")
        .build()
        .expect("build http client")
}

/// Current platform triple used by nodejs.org distributions.
fn node_dist_target() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win"
    } else {
        "linux"
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => panic!("unsupported architecture: {other}"),
    };
    format!("{os}-{arch}")
}

/// Pick the newest LTS Node (major >= 20) from nodejs.org, else the fallback.
async fn pick_node_version() -> String {
    const INDEX: &str = "https://nodejs.org/dist/index.json";
    if let Ok(resp) = http_client().get(INDEX).send().await {
        if let Ok(v) = resp.json::<Vec<serde_json::Value>>().await {
            for entry in &v {
                let Some(ver) = entry.get("version").and_then(|x| x.as_str()) else { continue };
                let lts = entry.get("lts").map(|x| !x.is_null()).unwrap_or(false);
                let major = ver
                    .trim_start_matches('v')
                    .split('.')
                    .next()
                    .and_then(|m| m.parse::<u32>().ok())
                    .unwrap_or(0);
                if lts && major >= 20 {
                    return ver.to_string();
                }
            }
        }
    }
    log(&format!(
        "nodejs.org dist index unreachable, falling back to {NODE_FALLBACK_VERSION}"
    ));
    NODE_FALLBACK_VERSION.to_string()
}

fn node_dist_url(version: &str) -> String {
    let target = node_dist_target();
    let file = if cfg!(windows) {
        format!("node-{version}-{target}.zip")
    } else {
        format!("node-{version}-{target}.tar.gz")
    };
    format!("https://nodejs.org/dist/{version}/{file}")
}

/// Extract the SHA-256 of `file` from nodejs.org SHASUMS256.txt.
async fn expected_sha256(version: &str, file: &str) -> Option<String> {
    let url = format!("https://nodejs.org/dist/{version}/SHASUMS256.txt");
    let resp = http_client().get(url).send().await.ok()?;
    let text = resp.text().await.ok()?;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        if name == format!("*{file}") || name == file {
            return Some(hash.to_lowercase());
        }
    }
    None
}

/// Download a URL to a file. Calls `on_progress(done, total)` while streaming.
async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    on_progress: Option<Box<dyn Fn(u64, u64) + Send>>,
) -> Result<(), String> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download {url}: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    use std::io::Write;
    let mut out = std::io::BufWriter::new(
        std::fs::File::create(dest).map_err(|e| format!("create {dest:?}: {e}"))?,
    );
    let mut got: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("download {url}: {e}"))?
    {
        out.write_all(&chunk).map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        if let Some(cb) = on_progress.as_ref() {
            cb(got, total);
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}

fn sha256_of_file(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(hex::encode(h.finalize()))
}

/// Locate the node executable inside an installed runtime directory.
/// Windows archives place `node.exe` at the runtime root; Unix archives use
/// `<root>/bin/node`. Both Windows layouts are accepted for robustness.
fn runtime_node_bin(dir: &std::path::Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        candidates.push(dir.join("node.exe"));
        candidates.push(dir.join("bin").join("node.exe"));
    } else {
        candidates.push(dir.join("bin").join("node"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Install the bundled Node.js runtime. Returns the installed version.
pub async fn install_node(on_progress: Option<Box<dyn Fn(u64, u64) + Send>>) -> Result<String, String> {
    let version = pick_node_version().await;
    let target = node_dist_target();
    let file = if cfg!(windows) {
        format!("node-{version}-{target}.zip")
    } else {
        format!("node-{version}-{target}.tar.gz")
    };
    let url = node_dist_url(&version);

    let Some(expected) = expected_sha256(&version, &file).await else {
        return Err(format!(
            "Could not verify the Node.js {version} download checksum. Please check your network connection and try again."
        ));
    };

    let root = runtime_dir();
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    // Clean up downloads left behind by previously interrupted installs.
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("node-dl-") && e.path().is_file() {
                log(&format!("removing stale download: {}", e.path().display()));
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let final_dir = root.join(format!("node-{version}-{target}"));
    if runtime_node_bin(&final_dir).is_some() {
        log(&format!("bundled node already present: {}", final_dir.display()));
        return Ok(version);
    }

    let tmp_file = root.join(format!("node-dl-{version}.{}", if cfg!(windows) { "zip" } else { "tar.gz" }));

    let client = http_client();
    log(&format!("downloading {url}"));
    download(&client, &url, &tmp_file, on_progress).await?;

    let actual = sha256_of_file(&tmp_file)?;
    if actual != expected {
        let _ = std::fs::remove_file(&tmp_file);
        return Err("Node.js download failed checksum verification. The file may have been corrupted or tampered with — please try again.".into());
    }
    log(&format!("sha256 verified for {file}"));

    // Extract.
    let bytes = std::fs::read(&tmp_file).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&tmp_file);
    let extract_root = root.join(format!("node-x-{version}"));
    let _ = std::fs::remove_dir_all(&extract_root);
    std::fs::create_dir_all(&extract_root).map_err(|e| e.to_string())?;
    if cfg!(windows) {
        extract_zip(&bytes, &extract_root).map_err(|e| format!("extract node: {e}"))?;
    } else {
        extract_targz(&bytes, &extract_root).map_err(|e| format!("extract node: {e}"))?;
    }

    // The archive contains a single top-level directory; move it into place.
    let entries: Vec<PathBuf> = std::fs::read_dir(&extract_root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let top = entries.into_iter().find(|p| p.is_dir());
    let Some(top) = top else {
        return Err("Unexpected Node.js archive layout (no top-level directory found).".into());
    };
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&top, &final_dir).map_err(|e| {
        format!("place runtime: {e} (try closing other launcher copies)")
    })?;
    let _ = std::fs::remove_dir_all(&extract_root);

    // Record provenance.
    let manifest = serde_json::json!({
        "version": version.trim_start_matches('v'),
        "url": url,
        "sha256": expected,
        "installed_at": crate::config::now_stamp(),
    });
    let _ = std::fs::write(final_dir.join("installed.json"), serde_json::to_string_pretty(&manifest).unwrap_or_default());

    // Smoke test.
    let Some(bin) = runtime_node_bin(&final_dir) else {
        return Err(
            "Node.js extraction finished but the node executable was not found in place."
                .into(),
        );
    };
    match std::process::Command::new(&bin).arg("--version").output() {
        Ok(o) if o.status.success() => {
            let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
            log(&format!("bundled node installed: {v} at {}", bin.display()));
            Ok(v.trim_start_matches('v').to_string())
        }
        Ok(o) => Err(format!(
            "Node.js installed but failed to run (exit {:?}): {}",
            o.status,
            String::from_utf8_lossy(&o.stderr)
        )),
        Err(e) => Err(format!("Node.js installed but failed to run: {e}")),
    }
}

fn extract_targz(bytes: &[u8], dest: &std::path::Path) -> Result<(), String> {
    use flate2::read::GzDecoder;
    let gz = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        // Sanity: refuse path traversal.
        let name = entry
            .path()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        if name.starts_with("..") || name.contains("..\\") {
            return Err(format!("unsafe path in archive: {name}"));
        }
        entry.unpack_in(dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn extract_zip(bytes: &[u8], dest: &std::path::Path) -> Result<(), String> {
    use std::io::{Read, Write};
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        if name.starts_with("..") {
            return Err(format!("unsafe path in archive: {name}"));
        }
        let out = dest.join(name);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        f.write_all(&buf).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DeepSeek Harness installation
// ---------------------------------------------------------------------------

const NPM_TIMEOUT_SECS: u64 = 10 * 60;

/// Install (or update) the DSH package using the given Node runtime, into
/// `dir` (the folder that will contain `node_modules/@deepseek-ai/dsh`).
/// Returns the installed version.
///
/// **Ordering guarantee:** DSH is installed with the npm that ships with the
/// Node runtime passed in — callers must make sure Node.js is installed and
/// detected *first*. We refuse to run without it.
pub async fn install_dsh(
    node: &std::path::Path,
    dir: &std::path::Path,
    on_tail: Option<Box<dyn Fn(String) + Send>>,
) -> Result<String, String> {
    if !node.exists() {
        return Err(format!(
            "Node.js runtime not found at {}. Install Node.js first.",
            node.display()
        ));
    }
    // Test-run the chosen runtime before invoking npm — a node that exists
    // but cannot execute (corrupt install, missing DLL, …) fails here with a
    // clear message instead of a confusing npm error later.
    match std::process::Command::new(node).arg("--version").output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            return Err(format!(
                "Node.js at {} does not run (--version, exit {:?}).",
                node.display(),
                o.status
            ));
        }
        Err(e) => {
            return Err(format!(
                "Node.js at {} cannot be executed: {e}",
                node.display()
            ));
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    // Minimal package.json so npm treats this as a project root.
    let manifest = dir.join("package.json");
    if !manifest.exists() {
        let pkg = serde_json::json!({
            "name": "dsh-launcher-runtime",
            "private": true,
            "version": "0.1.1"
        });
        std::fs::write(&manifest, serde_json::to_string_pretty(&pkg).unwrap_or_default())
            .map_err(|e| e.to_string())?;
    }

    let npm = npm_cli_for(node).ok_or_else(|| {
        format!(
            "Cannot locate npm for the selected Node runtime at {}.",
            node.display()
        )
    })?;

    log(&format!("running: {} install {DSH_PACKAGE}@latest", node.display()));

    // One attempt: spawn npm and wait (with a hard timeout). A timeout is a
    // hard error and is NOT retried — a hung network would otherwise eat
    // twice the timeout.
    let attempt = || async {
        let mut cmd = tokio::process::Command::new(node);
        cmd.arg(&npm)
            .arg("install")
            .arg("--prefix")
            .arg(dir)
            .arg("--no-audit")
            .arg("--no-fund")
            .arg("--loglevel=warn")
            .arg(format!("{DSH_PACKAGE}@latest"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd
            .spawn()
            .map_err(|e| format!("start npm: {e}"))?;
        tokio::time::timeout(
            Duration::from_secs(NPM_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| "npm install timed out after 10 minutes.".to_string())?
        .map_err(|e| format!("run npm: {e}"))
    };

    let is_transient = |combined: &str| {
        const TRANSIENT_MARKERS: [&str; 6] = [
            "ETIMEDOUT",
            "ECONNRESET",
            "ECONNREFUSED",
            "ENOTFOUND",
            "EAI_AGAIN",
            "socket hang up",
        ];
        combined.is_empty() || TRANSIENT_MARKERS.iter().any(|m| combined.contains(m))
    };

    let out = attempt().await?;
    // registry.npmjs.org is flaky on some networks; a second attempt right
    // after a network-ish failure usually succeeds (the old workaround was
    // "click install again" manually).
    let (out, retried) = if out.status.success() {
        (out, false)
    } else {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if is_transient(&combined) {
            log("npm install failed (likely network) — retrying once after 2s…");
            tokio::time::sleep(Duration::from_secs(2)).await;
            (attempt().await?, true)
        } else {
            (out, false)
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{stdout}{stderr}");
    let tail: String = combined.lines().rev().take(15).collect::<Vec<_>>().join("\n");

    if !out.status.success() {
        if let Some(cb) = on_tail {
            cb(tail.clone());
        }
        log(&format!("npm install failed:\n{tail}"));
        return Err(format!(
            "npm install failed. {}\n\nShow Details for the full npm output.",
            if combined.is_empty() {
                "No output was produced (check your network connection)."
            } else if retried {
                "A second attempt also failed — check the details for npm output."
            } else {
                "Check the details for npm output."
            }
        ));
    }

    let installed = detect_dsh_in(dir)
        .ok_or_else(|| "npm finished but the DSH package was not found afterwards.".to_string())?;
    if !dsh_bin_js_in(dir).exists() {
        return Err("npm finished but the DSH entry point is missing (unexpected package layout).".into());
    }
    log(&format!("DSH installed: v{} at {}", installed.version, dir.display()));
    Ok(installed.version)
}

/// Latest published version of DSH on the npm registry (abbreviated metadata).
pub async fn latest_dsh_version() -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get("https://registry.npmjs.org/@deepseek-ai/dsh")
        .header("accept", "application/vnd.npm.install-v1+json")
        .send()
        .await
        .map_err(|e| format!("npm registry unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("npm registry returned HTTP {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    // Abbreviated metadata has no top-level "version"; the latest tag lives
    // under "dist-tags.latest".
    v.get("dist-tags")
        .and_then(|t| t.get("latest"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "unexpected npm registry response".to_string())
}
