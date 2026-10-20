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

/// Resolve the exact local listening endpoint. Ambiguous matches are rejected.
pub fn listener_pid(host: &str, port: u16) -> Option<u32> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("lsof")
            .args([
                "-nP".into(),
                format!("-iTCP:{port}"),
                "-sTCP:LISTEN".into(),
                "-Fpn".into(),
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_lsof(&String::from_utf8_lossy(&out.stdout), host, port)
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("netstat")
            .args(["-ano", "-p", "tcp"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        parse_netstat(&String::from_utf8_lossy(&out.stdout), host, port)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (host, port);
        None
    }
}

fn endpoint_matches(endpoint: &str, host: &str, port: u16) -> bool {
    let Some((address, port_text)) = endpoint.rsplit_once(':') else {
        return false;
    };
    if port_text.parse::<u16>().ok() != Some(port) {
        return false;
    }
    let address = address.trim_matches(['[', ']']);
    let host = host.trim_matches(['[', ']']);
    let host_ip = if host == "localhost" {
        Some(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST))
    } else {
        host.parse::<std::net::IpAddr>().ok()
    };
    let Some(host_ip) = host_ip else {
        return false;
    };
    let wildcard = address == "*"
        || address
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_unspecified())
            .unwrap_or(false);
    // Never map a remote address to an unrelated local wildcard listener.
    if wildcard {
        return host_ip.is_loopback() || host_ip.is_unspecified();
    }
    let Ok(address_ip) = address.parse::<std::net::IpAddr>() else {
        return false;
    };
    address_ip == host_ip || (host == "localhost" && address_ip.is_loopback())
}

fn unique_pid(pids: impl Iterator<Item = u32>) -> Option<u32> {
    let mut pids = pids.filter(|pid| *pid != 0);
    let first = pids.next()?;
    pids.all(|pid| pid == first).then_some(first)
}

#[cfg(any(unix, test))]
fn parse_lsof(text: &str, host: &str, port: u16) -> Option<u32> {
    let mut current = None;
    let mut pids = Vec::new();
    for line in text.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid.parse().ok();
        }
        if let Some(endpoint) = line.strip_prefix('n') {
            if endpoint_matches(endpoint, host, port) {
                if let Some(pid) = current {
                    pids.push(pid);
                }
            }
        }
    }
    unique_pid(pids.into_iter())
}

#[cfg(any(windows, test))]
fn parse_netstat(text: &str, host: &str, port: u16) -> Option<u32> {
    unique_pid(text.lines().filter_map(|line| {
        let cols: Vec<_> = line.split_whitespace().collect();
        if cols.len() != 5
            || !cols[0].eq_ignore_ascii_case("TCP")
            || cols[3] != "LISTENING"
            || !endpoint_matches(cols[1], host, port)
        {
            return None;
        }
        cols[4].parse().ok()
    }))
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
    pid > 0 && pid <= i32::MAX as u32 && unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Send a graceful termination signal (SIGTERM).
#[cfg(unix)]
pub fn signal_terminate(pid: u32) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Force kill.
#[cfg(unix)]
pub fn signal_kill(pid: u32) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_listener_requires_exact_local_port_and_state() {
        let text = "TCP 127.0.0.1:30800 0.0.0.0:0 LISTENING 10\nTCP 127.0.0.1:3080 0.0.0.0:0 LISTENING 20\nTCP 127.0.0.1:5000 127.0.0.1:3080 ESTABLISHED 30";
        assert_eq!(parse_netstat(text, "127.0.0.1", 3080), Some(20));
    }

    #[test]
    fn listeners_require_exact_address_and_reject_ambiguity() {
        let text = "p10\nn127.0.0.2:3080\np20\nn127.0.0.1:3080\n";
        assert_eq!(parse_lsof(text, "127.0.0.1", 3080), Some(20));
        assert_eq!(
            parse_lsof("p10\nn*:3080\np20\nn127.0.0.1:3080", "127.0.0.1", 3080),
            None
        );
        assert_eq!(
            parse_lsof("p20\nn*:3080\nn[::1]:3080", "localhost", 3080),
            Some(20)
        );
    }

    #[test]
    fn ipv6_and_wildcard_listeners_are_supported_without_mapping_remote_hosts() {
        assert!(endpoint_matches("[::1]:3080", "::1", 3080));
        assert!(endpoint_matches("[::]:3080", "::1", 3080));
        assert!(endpoint_matches("0.0.0.0:3080", "127.0.0.1", 3080));
        assert!(!endpoint_matches("*:3080", "192.0.2.1", 3080));
        assert!(!endpoint_matches("127.0.0.1:30800", "127.0.0.1", 3080));
        assert_eq!(
            parse_netstat("TCP [::1]:3080 [::]:0 LISTENING 20", "::1", 3080),
            Some(20)
        );
    }

    #[test]
    fn invalid_pids_are_never_alive() {
        assert!(!process_alive(0));
        #[cfg(unix)]
        assert!(!process_alive(u32::MAX));
    }
}
