//! Identify a local DSH process independently of its HTTP response or PID alone.

use std::path::{Path, PathBuf};
use sysinfo::{Pid, ProcessRefreshKind, System, UpdateKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub start_time: u64,
    exe: PathBuf,
    script: PathBuf,
}

pub fn read(pid: u32) -> Option<ProcessIdentity> {
    if pid == 0 || pid == std::process::id() {
        return None;
    }
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_process_specifics(
        pid,
        ProcessRefreshKind::new()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .with_cwd(UpdateKind::Always),
    );
    let process = system.process(pid)?;
    let exe = process.exe()?.canonicalize().ok()?;
    // Custom Node overrides may use a different executable filename. The
    // actual DSH entry point identifies the service; exe + creation time
    // bind that identity to this particular OS process.
    let script = dsh_entry(process.cmd(), process.cwd())?;
    let start_time = process.start_time();
    (start_time != 0).then_some(ProcessIdentity {
        start_time,
        exe,
        script,
    })
}

/// Validate the actual Node entry point, never a substring elsewhere in argv.
fn dsh_entry(args: &[String], cwd: Option<&Path>) -> Option<PathBuf> {
    let mut index = 1;
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--" => {
                index += 1;
                break;
            }
            "--require" | "-r" | "--import" | "--loader" | "--experimental-loader" => index += 2,
            "--no-warnings" | "--enable-source-maps" | "--experimental-strip-types" => index += 1,
            arg if arg.starts_with("--inspect")
                || arg.starts_with("--require=")
                || arg.starts_with("--import=")
                || arg.starts_with("--loader=") =>
            {
                index += 1
            }
            _ => break,
        }
    }
    let entry = Path::new(args.get(index)?);
    let entry = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        cwd?.join(entry)
    };
    let script = entry.canonicalize().ok()?;
    if script.file_name()? != "bin.js" || script.parent()?.file_name()? != "lib" {
        return None;
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(script.parent()?.parent()?.join("package.json")).ok()?,
    )
    .ok()?;
    if manifest.get("name")?.as_str()? != "@deepseek-ai/dsh" {
        return None;
    }
    let app_args = &args[index + 1..];
    let web = app_args.first().map(String::as_str) == Some("web")
        || app_args
            .windows(2)
            .any(|pair| pair[0] == "--profile" && pair[1] == "web")
        || app_args.iter().any(|arg| arg == "--profile=web");
    web.then_some(script)
}

pub fn matches(pid: u32, expected: &ProcessIdentity) -> bool {
    read(pid).as_ref() == Some(expected)
}
