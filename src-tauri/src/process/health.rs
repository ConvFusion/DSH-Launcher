//! Health checking.
//!
//! Startup completion is **never** decided by sleeping a fixed time: we poll
//! the HTTP endpoint until it answers (or the deadline expires), so the
//! experience is stable on fast and slow machines alike.

use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("build health client")
    })
}

/// Returns true when the harness answers HTTP on the given host/port.
pub async fn http_ok(host: &str, port: u16) -> bool {
    let url = format!("http://{host}:{port}/");
    match client().get(&url).send().await {
        Ok(resp) => {
            let s = resp.status().as_u16();
            (200..500).contains(&s)
        }
        Err(_) => false,
    }
}

/// Poll until the endpoint answers or the deadline passes.
pub async fn wait_ready(host: &str, port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if http_ok(host, port).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Err(format!(
        "DeepSeek Harness did not become ready within {} seconds.",
        timeout.as_secs()
    ))
}

/// True when something already listens on the port.
pub fn port_in_use(host: &str, port: u16) -> bool {
    let addr = std::net::SocketAddr::new(
        if host == "127.0.0.1" || host == "localhost" {
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            match host.parse() {
                Ok(a) => a,
                Err(_) => return false,
            }
        },
        port,
    );
    TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
}

/// Best-effort pid of the process currently listening on `port` (macOS /
/// Linux via `lsof`, Windows via `netstat`). Used to adopt an instance that
/// was started outside the launcher.
pub fn listener_pid(_host: &str, port: u16) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::process::Command;
        let out = Command::new("lsof")
            .args([format!("-tiTCP:{port}"), "-sTCP:LISTEN".into()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .and_then(|l| l.trim().parse().ok())
    }
    #[cfg(windows)]
    {
        use std::process::Command;
        let out = Command::new("netstat").arg("-ano").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let needle = format!(":{port}");
        for line in text.lines() {
            if line.contains(&needle) && line.contains("LISTENING") {
                if let Some(pid) = line.rsplit_whitespace().next() {
                    if let Ok(p) = pid.parse() {
                        return Some(p);
                    }
                }
            }
        }
        None
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = port;
        None
    }
}

/// Suggest up to `count` free ports starting just after `preferred`.
pub fn suggest_ports(preferred: u16, count: usize) -> Vec<u16> {
    let mut out = Vec::new();
    for p in (preferred + 1)..=(preferred.saturating_add(20)) {
        if !port_in_use("127.0.0.1", p) {
            out.push(p);
            if out.len() >= count {
                break;
            }
        }
    }
    out
}

/// Is a process with this pid alive?
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Send a graceful termination signal (SIGTERM).
#[cfg(unix)]
pub fn signal_terminate(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Force kill.
#[cfg(unix)]
pub fn signal_kill(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn handle_for(pid: u32) -> *mut std::ffi::c_void {
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }
    }

    pub fn alive(pid: u32) -> bool {
        let h = handle_for(pid);
        if h.is_null() {
            return false;
        }
        unsafe { CloseHandle(h) };
        true
    }

    pub fn terminate(pid: u32, code: u32) {
        // PROCESS_TERMINATE = 0x0001
        const PROCESS_TERMINATE: u32 = 0x0001;
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, code);
                CloseHandle(h);
            }
        }
    }
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    win::alive(pid)
}

#[cfg(windows)]
pub fn signal_terminate(pid: u32) {
    win::terminate(pid, 1);
}

#[cfg(windows)]
pub fn signal_kill(pid: u32) {
    win::terminate(pid, 1);
}
