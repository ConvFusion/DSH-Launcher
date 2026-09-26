//! Installing missing pieces:
//!
//! * Node.js — downloaded from nodejs.org (official, SHA-256 verified) into
//!   `~/.dsh-launcher/runtime/`. The user's system Node is never touched.
//! * DeepSeek Harness — `npm install @deepseek-ai/dsh` into `~/.dsh-launcher/dsh`.

use super::detector::{
    dsh_bin_js_in, detect_dsh_in, node_compatible, npm_cli_for, DSH_PACKAGE,
};
use crate::config::{install_log_line, log, runtime_dir};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Fallback Node version used when nodejs.org cannot be reached for its index.
///
/// Must itself satisfy [`node_compatible`] — the old `v22.14.0` did not, so an
/// offline machine would have been handed a runtime that installs DSH and then
/// cannot execute it.
pub const NODE_FALLBACK_VERSION: &str = "v22.19.0";

/// Receives every output line of an install/update command **as it is
/// produced**, so the UI can show the live console log instead of waiting for
/// npm to exit. `Arc` because the stdout and stderr reader tasks share it.
pub type LineSink = Arc<dyn Fn(String) + Send + Sync>;

/// Push one line to a sink, if the caller supplied one.
fn emit_line(sink: &Option<LineSink>, line: impl Into<String>) {
    if let Some(cb) = sink.as_ref() {
        cb(line.into());
    }
}

/// Read a child pipe line by line, appending the full transcript to `buf`
/// (that is what `install.log` records) while forwarding each line to the UI
/// sink live. Returns the task handle so the caller can drain it before
/// judging the transcript.
fn spawn_line_reader<R>(
    pipe: R,
    buf: Arc<Mutex<String>>,
    sink: Option<LineSink>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(pipe).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            {
                let mut b = buf.lock().unwrap();
                b.push_str(&line);
                b.push('\n');
            }
            emit_line(&sink, line);
        }
    })
}

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

/// Pick the newest LTS Node **that can run DSH** from nodejs.org, else the
/// fallback. nodejs.org's index is newest-first, so the first match is the
/// newest compatible release.
async fn pick_node_version() -> String {
    const INDEX: &str = "https://nodejs.org/dist/index.json";
    if let Ok(resp) = http_client().get(INDEX).send().await {
        if let Ok(v) = resp.json::<Vec<serde_json::Value>>().await {
            for entry in &v {
                let Some(ver) = entry.get("version").and_then(|x| x.as_str()) else { continue };
                let lts = entry.get("lts").map(|x| !x.is_null()).unwrap_or(false);
                if lts && node_compatible(ver.trim_start_matches('v')) {
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
///
/// Key milestones (download, checksum, extraction, smoke test) are mirrored
/// to `install.log`, and **any** failure — including download errors — lands
/// there with a `node install FAILED:` marker so a broken machine can be
/// diagnosed from the log bundle alone.
pub async fn install_node(on_progress: Option<Box<dyn Fn(u64, u64) + Send>>) -> Result<String, String> {
    let res = install_node_inner(on_progress).await;
    if let Err(e) = &res {
        install_log_line(&format!("node install FAILED: {e}"));
    }
    res
}

async fn install_node_inner(on_progress: Option<Box<dyn Fn(u64, u64) + Send>>) -> Result<String, String> {
    let version = pick_node_version().await;
    let target = node_dist_target();
    let file = if cfg!(windows) {
        format!("node-{version}-{target}.zip")
    } else {
        format!("node-{version}-{target}.tar.gz")
    };
    let url = node_dist_url(&version);
    install_log_line(&format!(
        "node install start: version={version} url={url}"
    ));

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
    install_log_line(&format!("sha256 verified for {file}"));

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
            install_log_line(&format!("node installed: {v} at {}", bin.display()));
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

/// Dependencies whose lifecycle scripts the install genuinely needs.
///
/// npm (11.19+) only runs a dependency's `install`/`postinstall` when an
/// explicit `allowScripts` policy covers it; anything uncovered is silently
/// skipped as "unreviewed". Two of DSH's dependencies need theirs: node-pty
/// builds/stages its native prebuilds, and
/// `@deepseek-ai/dsh-subprocess-local` ships a postinstall that restores the
/// executable bit npm's tarball handling strips from node-pty's prebuilt
/// `spawn-helper` — without it the helper is not executable and local
/// subprocesses break.
///
/// A user-level `.npmrc` that restricts scripts to an unrelated allowlist
/// (e.g. `allow-scripts=some-other-pkg`) would otherwise skip both and leave a
/// subtly broken install. Declaring them in the **project** `package.json` is
/// what npm's own error prescribes, and per npm's precedence rules the project
/// package.json layer wins over `.npmrc`.
///
/// If a future DSH release adds another script-bearing dependency it will be
/// skipped with npm's advisory warning rather than failing the install — the
/// warning shows up in the update log, so it stays visible.
const DSH_SCRIPT_PACKAGES: [&str; 2] = [
    "node-pty",
    "@deepseek-ai/dsh-subprocess-local",
];

/// Environment variables npm exports to child processes to carry its resolved
/// configuration. The launcher may itself have been started from an npm script
/// (`npm run tauri dev`), and leaking these into the DSH install makes npm read
/// the inherited `allow-scripts` as a *command-line* flag — which it rejects
/// outright in a project-scoped install:
///
/// ```text
/// npm error code EALLOWSCRIPTS
/// npm error --allow-scripts is not allowed in project-scoped installs.
/// ```
///
/// Only the script-policy keys are stripped, so a registry or proxy the user
/// deliberately exported still reaches the child; and `.npmrc` is untouched, so
/// their real configuration is still honoured.
const NPM_SCRIPT_POLICY_VARS: [&str; 5] = [
    "npm_config_allow_scripts",
    "npm_config_ignore_scripts",
    "npm_config_strict_allow_scripts",
    "npm_config_allow_scripts_pin",
    "npm_config_dangerously_allow_all_scripts",
];

/// Create/repair the install root's `package.json`, preserving whatever is
/// already there (npm's npx cache keeps its own `_npx` bookkeeping in this
/// file) and merging in the `allowScripts` policy from [`DSH_SCRIPT_PACKAGES`].
fn ensure_install_manifest(manifest: &std::path::Path) -> Result<(), String> {
    let mut doc: serde_json::Value = std::fs::read_to_string(manifest)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));

    let obj = doc.as_object_mut().expect("checked above");
    // Identity of the launcher-managed project root — only filled in when the
    // file did not exist (an npx-cache manifest keeps its own name/version).
    obj.entry("name")
        .or_insert_with(|| serde_json::json!("dsh-launcher-runtime"));
    obj.entry("private").or_insert_with(|| serde_json::json!(true));
    obj.entry("version").or_insert_with(|| serde_json::json!("0.1.1"));

    let allow = obj
        .entry("allowScripts")
        .or_insert_with(|| serde_json::json!({}));
    if !allow.is_object() {
        // An empty array (the shape our own project uses) means "no policy";
        // replace it with the map form npm actually reads.
        *allow = serde_json::json!({});
    }
    let map = allow.as_object_mut().expect("just ensured");
    for name in DSH_SCRIPT_PACKAGES {
        map.entry(name.to_string())
            .or_insert_with(|| serde_json::json!(true));
    }

    std::fs::write(
        manifest,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&doc).unwrap_or_default()
        ),
    )
    .map_err(|e| format!("write {}: {e}", manifest.display()))
}

/// Install (or update) the DSH package using the given Node runtime, into
/// `dir` (the folder that will contain `node_modules/@deepseek-ai/dsh`).
/// Returns the installed version.
///
/// `version` is the exact build to install. `None` resolves the newest build
/// published on any npm channel, which is what a first install wants; the
/// update buttons pass an explicit version so the version the UI advertised is
/// the version that actually lands on disk (npm's `latest` tag is pinned to
/// the maintainers' *recommended* build and can sit well behind the newest).
///
/// **Ordering guarantee:** DSH is installed with the npm that ships with the
/// Node runtime passed in — callers must make sure Node.js is installed and
/// detected *first*. We refuse to run without it.
///
/// `on_line` receives the command line and then **every** npm output line as
/// it is produced, which is what lets the UI show a live console log during an
/// install/update. `on_fail_tail` is the older hook: the last npm lines, handed
/// over only when the command failed (used to fill in "Show Details").
pub async fn install_dsh(
    node: &std::path::Path,
    dir: &std::path::Path,
    version: Option<&str>,
    on_line: Option<LineSink>,
    on_fail_tail: Option<Box<dyn Fn(String) + Send>>,
) -> Result<String, String> {
    if !node.exists() {
        let e = format!(
            "Node.js runtime not found at {}. Install Node.js first.",
            node.display()
        );
        install_log_line(&format!("dsh install FAILED: {e}"));
        return Err(e);
    }
    // Test-run the chosen runtime before invoking npm — a node that exists
    // but cannot execute (corrupt install, missing DLL, …) fails here with a
    // clear message instead of a confusing npm error later.
    match std::process::Command::new(node).arg("--version").output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let e = format!(
                "Node.js at {} does not run (--version, exit {:?}).",
                node.display(),
                o.status
            );
            install_log_line(&format!("dsh install FAILED: {e}"));
            return Err(e);
        }
        Err(e) => {
            let e = format!(
                "Node.js at {} cannot be executed: {e}",
                node.display()
            );
            install_log_line(&format!("dsh install FAILED: {e}"));
            return Err(e);
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    // The install root is a project (npm needs a package.json to treat the
    // `--prefix` directory as its project root), and it carries the
    // `allowScripts` policy the install needs — see `ensure_install_manifest`.
    let manifest = dir.join("package.json");
    ensure_install_manifest(&manifest)?;

    let npm = npm_cli_for(node).ok_or_else(|| {
        let e = format!(
            "Cannot locate npm for the selected Node runtime at {}.",
            node.display()
        );
        install_log_line(&format!("dsh install FAILED: {e}"));
        e
    })?;

    install_log_line(&format!(
        "dsh install start: node={} npm-cli={} prefix={}",
        node.display(),
        npm.display(),
        dir.display()
    ));

    // npm runs dependency lifecycle scripts through `sh -c`, so **`node` must
    // be resolvable by name** — not just as the absolute path we spawn npm
    // with. DSH ships packages that rely on this (node-pty's `install`, and
    // `@deepseek-ai/dsh-subprocess-local`'s `postinstall:
    // node scripts/ensure-spawn-helper.mjs`). A GUI-launched app inherits a
    // minimal PATH (`/usr/bin:/bin:…`) where an nvm or Homebrew Node.js is
    // invisible, so those scripts died with `sh: node: command not found`
    // (exit 127) and aborted the 0.1.5 upgrade. Prepend the runtime we chose —
    // the same thing `ProcessManager::start` and the plugin installer do.
    let node_dir = node
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let old_path = std::env::var("PATH").unwrap_or_default();
    let child_path = format!("{}{}{old_path}", node_dir.display(), sep);

    install_log_line(&format!(
        "install root manifest: {} (allowScripts: {})",
        manifest.display(),
        DSH_SCRIPT_PACKAGES.join(", ")
    ));

    // Pin the exact version being installed. The home page advertises a
    // specific version per channel, so letting npm re-resolve a tag here could
    // install something other than what was promised — and the installed
    // version would then never reach the advertised one, leaving "update
    // available" on screen forever.
    let spec = match version {
        Some(v) => format!("{DSH_PACKAGE}@{v}"),
        None => match latest_dsh_version().await {
            Ok(v) => format!("{DSH_PACKAGE}@{v}"),
            Err(e) => {
                install_log_line(&format!(
                    "could not resolve the newest DSH version ({e}) — falling back to the npm `latest` tag"
                ));
                format!("{DSH_PACKAGE}@latest")
            }
        },
    };

    log(&format!("running: {} install {spec}", node.display()));

    // Show the exact command as the first line of the live log, so the user
    // sees what is running (and can paste it into a terminal if it fails).
    emit_line(
        &on_line,
        format!(
            "$ {} {} install --prefix {} --no-audit --no-fund --loglevel=warn {spec}",
            node.display(),
            npm.display(),
            dir.display()
        ),
    );

    // One attempt: spawn npm and wait (with a hard timeout). A timeout is a
    // hard error and is NOT retried — a hung network would otherwise eat
    // twice the timeout.
    //
    // npm's stdout/stderr are read **line by line while it runs** and pushed
    // to `on_line`, so the UI shows progress instead of a frozen spinner for
    // the minutes a cold install can take. The same lines are accumulated so
    // the full transcript still lands in install.log exactly as before.
    let attempt = || async {
        let mut cmd = tokio::process::Command::new(node);
        cmd.arg(&npm)
            .arg("install")
            .arg("--prefix")
            .arg(dir)
            .arg("--no-audit")
            .arg("--no-fund")
            .arg("--loglevel=warn")
            .arg(&spec)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // Node.js must be on PATH for npm's own lifecycle scripts — see above.
        cmd.env("PATH", &child_path);
        // Drop any npm script policy inherited from whoever launched us, so the
        // policy that applies is the one in the install root's package.json.
        for (key, _) in std::env::vars() {
            let lower = key.to_ascii_lowercase();
            if NPM_SCRIPT_POLICY_VARS.contains(&lower.as_str()) {
                cmd.env_remove(&key);
            }
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("start npm: {e}"))?;

        let stdout_buf = Arc::new(Mutex::new(String::new()));
        let stderr_buf = Arc::new(Mutex::new(String::new()));
        let mut readers = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            readers.push(spawn_line_reader(
                pipe,
                Arc::clone(&stdout_buf),
                on_line.clone(),
            ));
        }
        if let Some(pipe) = child.stderr.take() {
            readers.push(spawn_line_reader(
                pipe,
                Arc::clone(&stderr_buf),
                on_line.clone(),
            ));
        }

        let status = match tokio::time::timeout(
            Duration::from_secs(NPM_TIMEOUT_SECS),
            child.wait(),
        )
        .await
        {
            Err(_) => {
                // Don't leave a stray npm behind on a hung network.
                let _ = child.kill().await;
                emit_line(
                    &on_line,
                    format!("[launcher] npm install timed out after {NPM_TIMEOUT_SECS}s — stopped."),
                );
                return Err("npm install timed out after 10 minutes.".to_string());
            }
            Ok(Err(e)) => return Err(format!("run npm: {e}")),
            Ok(Ok(s)) => s,
        };

        // The pipes reach EOF when the child exits; drain the readers so the
        // transcript is complete before it is judged and written to the log.
        for r in readers {
            let _ = r.await;
        }

        let stdout = stdout_buf.lock().unwrap().clone();
        let stderr = stderr_buf.lock().unwrap().clone();
        // The full command output goes to install.log on EVERY attempt —
        // success or failure — so a broken machine can be diagnosed from the
        // log bundle without asking the user to read the console window.
        install_log_line(&format!(
            "npm exit: {:?}\n---- npm stdout ----\n{}\n---- npm stderr ----\n{}\n---- end npm output ----",
            status.code(),
            stdout,
            stderr,
        ));
        // Turbofish pins the closure's error type (String) - a bare Ok(out)
        // leaves it uninferrable from the call sites.
        Ok::<std::process::Output, String>(std::process::Output {
            status,
            stdout: stdout.into_bytes(),
            stderr: stderr.into_bytes(),
        })
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
            install_log_line("npm install failed (likely network) — retrying once after 2s…");
            emit_line(
                &on_line,
                "[launcher] npm install failed (likely network) — retrying once…",
            );
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
        if let Some(cb) = on_fail_tail {
            cb(tail.clone());
        }
        log(&format!("npm install failed:\n{tail}"));
        install_log_line("dsh install FAILED — full npm output above");
        emit_line(
            &on_line,
            format!(
                "[launcher] npm install failed (exit {:?}) — see the lines above.",
                out.status.code()
            ),
        );
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
    install_log_line(&format!(
        "DSH installed: v{} at {}",
        installed.version,
        dir.display()
    ));
    emit_line(
        &on_line,
        format!("[launcher] DeepSeek Harness v{} installed.", installed.version),
    );
    Ok(installed.version)
}

/// Highest semver among the values of an npm `dist-tags` map.
///
/// `dist-tags` maps a release **channel** to a version (`latest`, `next`,
/// `alpha`, …). Values that are not parseable versions are ignored, because a
/// tag is allowed to point at a non-version string.
fn newest_dist_tag_version(tags: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let mut newest: Option<(semver::Version, String)> = None;
    for value in tags.values() {
        let Some(raw) = value.as_str() else { continue };
        let Ok(parsed) = semver::Version::parse(raw) else {
            continue;
        };
        // `map_or` rather than `is_none_or`: keeps the crate's 1.77.2 MSRV.
        if newest.as_ref().map_or(true, |(best, _)| parsed > *best) {
            newest = Some((parsed, raw.to_string()));
        }
    }
    newest.map(|(_, raw)| raw)
}

/// The two DSH builds the launcher offers, as published on npm.
///
/// npm exposes release **channels** as dist-tags. The maintainers keep
/// `latest` pinned to the build they recommend, while newer builds ship under
/// the other tags (`next`, `alpha`) — so the two are routinely different, e.g.
/// `latest` = `0.1.5-rc.3` while `next` = `0.1.7-rc.2`. The launcher shows both
/// and lets the user choose, instead of pretending there is one answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DshChannels {
    /// What `dist-tags.latest` points at — the maintainers' recommended build.
    pub recommended: Option<String>,
    /// Highest version across every dist-tag — the newest published build.
    pub newest: Option<String>,
}

/// Turn an npm `dist-tags` map into the two channels the launcher offers.
///
/// `latest` is the maintainers' recommended channel; every other tag is
/// considered when hunting for the newest build. A tag that points at a
/// non-version string is ignored rather than trusted.
fn channels_from_dist_tags(
    tags: &serde_json::Map<String, serde_json::Value>,
) -> DshChannels {
    DshChannels {
        recommended: tags
            .get("latest")
            .and_then(|x| x.as_str())
            .filter(|s| semver::Version::parse(s).is_ok())
            .map(|s| s.to_string()),
        newest: newest_dist_tag_version(tags),
    }
}

/// Read every release channel DSH publishes, in a single registry request.
pub async fn dsh_channels() -> Result<DshChannels, String> {
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
    // Abbreviated metadata has no top-level "version"; the channel pointers
    // live under "dist-tags".
    let tags = v
        .get("dist-tags")
        .and_then(|t| t.as_object())
        .ok_or_else(|| "unexpected npm registry response (no dist-tags)".to_string())?;
    Ok(channels_from_dist_tags(tags))
}

/// Newest published version of DSH on the npm registry (abbreviated metadata).
///
/// Deliberately **not** `dist-tags.latest`, which the maintainers pin to the
/// recommended build: reading it alone made the launcher advertise a stale
/// version forever (it offered `0.1.5-rc.3` while `0.1.7-rc.2` was already
/// published under `next`). This is the version used when the caller does not
/// name a channel, e.g. a first install.
pub async fn latest_dsh_version() -> Result<String, String> {
    dsh_channels()
        .await?
        .newest
        .ok_or_else(|| "no parseable version among the npm dist-tags".to_string())
}

#[cfg(test)]
mod tests {
    use super::{channels_from_dist_tags, newest_dist_tag_version};

    fn tags(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect()
    }

    #[test]
    fn the_live_registry_maps_to_two_distinct_channels() {
        // The registry state that motivated showing both buttons: `latest` is
        // pinned to the maintainers' recommended build while `next` already
        // carries a newer one, so neither can stand in for the other.
        let t = tags(&[
            ("latest", "0.1.5-rc.3"),
            ("next", "0.1.7-rc.2"),
            ("alpha", "0.1.7-alpha.2"),
        ]);
        let ch = channels_from_dist_tags(&t);
        assert_eq!(ch.recommended.as_deref(), Some("0.1.5-rc.3"));
        assert_eq!(ch.newest.as_deref(), Some("0.1.7-rc.2"));
    }

    #[test]
    fn a_single_channel_reports_the_same_version_twice() {
        // When nothing newer exists the UI must collapse the two buttons into
        // one, which it does by comparing these two fields.
        let ch = channels_from_dist_tags(&tags(&[("latest", "0.1.7")]));
        assert_eq!(ch.recommended.as_deref(), Some("0.1.7"));
        assert_eq!(ch.newest.as_deref(), Some("0.1.7"));
    }

    #[test]
    fn a_recommended_tag_pointing_at_a_non_version_is_dropped() {
        let ch = channels_from_dist_tags(&tags(&[("latest", "beta"), ("next", "0.1.6-alpha.2")]));
        assert_eq!(ch.recommended, None);
        assert_eq!(ch.newest.as_deref(), Some("0.1.6-alpha.2"));
    }

    #[test]
    fn newest_dist_tag_beats_a_stale_latest_tag() {
        // `latest` pinned at 0.1.5-rc.3 while `next` already pointed at
        // 0.1.7-rc.2: reading only `latest` made the launcher advertise the
        // stale version forever.
        let t = tags(&[
            ("latest", "0.1.5-rc.3"),
            ("next", "0.1.7-rc.2"),
            ("alpha", "0.1.7-alpha.2"),
        ]);
        assert_eq!(newest_dist_tag_version(&t).as_deref(), Some("0.1.7-rc.2"));
    }

    #[test]
    fn a_final_release_outranks_its_own_release_candidate() {
        // semver orders 0.1.7 above 0.1.7-rc.2; channel order must not override.
        let t = tags(&[("latest", "0.1.7"), ("next", "0.1.7-rc.2")]);
        assert_eq!(newest_dist_tag_version(&t).as_deref(), Some("0.1.7"));
    }

    #[test]
    fn unparseable_tag_values_are_skipped() {
        // A dist-tag is allowed to point at a non-version string.
        let t = tags(&[("latest", "beta"), ("next", "0.1.6-alpha.2")]);
        assert_eq!(newest_dist_tag_version(&t).as_deref(), Some("0.1.6-alpha.2"));
    }

    #[test]
    fn no_parseable_tag_yields_none() {
        let t = tags(&[("latest", "not-a-version")]);
        assert_eq!(newest_dist_tag_version(&t), None);
    }
}
