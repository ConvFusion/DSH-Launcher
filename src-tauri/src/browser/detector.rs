//! Browser detection for macOS, Windows and Linux.

use crate::config::log;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BrowserId {
    Chrome,
    Edge,
    Firefox,
    Safari,
    Brave,
}

impl BrowserId {
    pub fn as_str(&self) -> &'static str {
        match self {
            BrowserId::Chrome => "chrome",
            BrowserId::Edge => "edge",
            BrowserId::Firefox => "firefox",
            BrowserId::Safari => "safari",
            BrowserId::Brave => "brave",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "chrome" => Some(BrowserId::Chrome),
            "edge" => Some(BrowserId::Edge),
            "firefox" => Some(BrowserId::Firefox),
            "safari" => Some(BrowserId::Safari),
            "brave" => Some(BrowserId::Brave),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            BrowserId::Chrome => "Google Chrome",
            BrowserId::Edge => "Microsoft Edge",
            BrowserId::Firefox => "Firefox",
            BrowserId::Safari => "Safari",
            BrowserId::Brave => "Brave",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserInfo {
    pub id: String,
    pub name: String,
    /// True when the browser is installed on this machine.
    pub installed: bool,
}

/// All browsers the launcher knows about, with install status.
pub fn list_browsers() -> Vec<BrowserInfo> {
    let list: Vec<BrowserInfo> = [
        BrowserId::Chrome,
        BrowserId::Edge,
        BrowserId::Firefox,
        BrowserId::Safari,
        BrowserId::Brave,
    ]
    .iter()
    .map(|id| BrowserInfo {
        id: id.as_str().to_string(),
        name: id.display_name().to_string(),
        installed: installed_path(*id).is_some(),
    })
    .collect();
    // Nothing found at all is almost always a detection problem — leave a
    // trace of what was checked so it is diagnosable from launcher.log.
    if list.iter().all(|b| !b.installed) {
        log(&format!(
            "browser detection failed: no supported browser found ({})",
            browser_diagnostics()
        ));
    }
    list
}

/// Human-readable summary of the places browser detection looks (diagnostics).
fn browser_diagnostics() -> String {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        format!(
            "checked /Applications/{{Google Chrome, Microsoft Edge, Firefox, Safari, Brave Browser}}.app and {home}/Applications"
        )
    }
    #[cfg(windows)]
    {
        let pf = std::env::var("ProgramFiles").unwrap_or_default();
        let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_default();
        let la = std::env::var("LOCALAPPDATA").unwrap_or_default();
        format!(
            "checked registry App Paths and ProgramFiles={pf}, ProgramFiles(x86)={pf86}, LOCALAPPDATA={la}"
        )
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        format!(
            "checked PATH ({}) and /usr/bin, /usr/local/bin, /snap/bin",
            std::env::var("PATH").unwrap_or_default()
        )
    }
}

/// Executable / app location for an installed browser, if any.
pub fn installed_path(id: BrowserId) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let names: &[&str] = match id {
            BrowserId::Chrome => &["/Applications/Google Chrome.app"],
            BrowserId::Edge => &["/Applications/Microsoft Edge.app"],
            BrowserId::Firefox => &["/Applications/Firefox.app"],
            BrowserId::Safari => &["/Applications/Safari.app"],
            BrowserId::Brave => &["/Applications/Brave Browser.app"],
        };
        // Also accept user-local /Applications.
        let user_apps = std::env::var("HOME")
            .ok()
            .map(|h| format!("{h}/Applications"));
        for n in names {
            let p = PathBuf::from(n);
            if p.exists() {
                return Some(p);
            }
            if let Some(dir) = &user_apps {
                let p = PathBuf::from(dir).join(p.file_name().unwrap());
                if p.exists() {
                    return Some(p);
                }
            }
        }
        None
    }
    #[cfg(windows)]
    {
        // 1. Registry App Paths — written by every official installer,
        //    including per-user installs, and always absolute.
        if id != BrowserId::Safari {
            let exe = match id {
                BrowserId::Chrome => "chrome.exe",
                BrowserId::Edge => "msedge.exe",
                BrowserId::Firefox => "firefox.exe",
                BrowserId::Brave => "brave.exe",
                BrowserId::Safari => unreachable!(),
            };
            if let Some(p) = windows_app_paths(exe) {
                return Some(p);
            }
        }
        // 2. Fallback: conventional install roots.
        let envs: &[&str] = &["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"];
        let rel: &[&str] = match id {
            BrowserId::Chrome => &["Google/Chrome/Application/chrome.exe"],
            BrowserId::Edge => &["Microsoft/Edge/Application/msedge.exe"],
            BrowserId::Firefox => &["Mozilla Firefox/firefox.exe"],
            BrowserId::Safari => &[],
            BrowserId::Brave => &["BraveSoftware/Brave-Browser/Application/brave.exe"],
        };
        for var in envs {
            let Ok(base) = std::env::var(var) else { continue };
            for r in rel {
                let p = PathBuf::from(base).join(r);
                if p.exists() {
                    return Some(p);
                }
            }
        }
        None
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // Linux & co: browsers are plain executables on PATH or under the
        // standard system prefixes.
        let names: &[&str] = match id {
            BrowserId::Chrome => &["google-chrome-stable", "google-chrome"],
            BrowserId::Edge => &["microsoft-edge-stable", "microsoft-edge"],
            BrowserId::Firefox => &["firefox"],
            BrowserId::Safari => &[],
            BrowserId::Brave => &["brave-browser", "brave"],
        };
        // 1. PATH scan.
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                for n in names {
                    let p = dir.join(n);
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
        // 2. Standard prefixes (GUI apps get a minimal PATH).
        for dir in ["/usr/bin", "/usr/local/bin", "/snap/bin"] {
            for n in names {
                let p = PathBuf::from(dir).join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }
}

/// Windows: resolve a browser executable from the `App Paths` registry key
/// that official installers register (HKLM for per-machine, HKCU for
/// per-user installs).
#[cfg(windows)]
fn windows_app_paths(exe: &str) -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let subkey =
        format!(r"Software\Microsoft\Windows\CurrentVersion\App Paths\{exe}");
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

/// The system's default browser (best effort).
pub fn default_browser() -> Option<BrowserId> {
    #[cfg(target_os = "macos")]
    {
        default_browser_macos()
    }
    #[cfg(windows)]
    {
        default_browser_windows()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn default_browser_macos() -> Option<BrowserId> {
    use std::process::Command;
    let out = Command::new("defaults")
        .args([
            "read",
            "com.apple.LaunchServices/com.apple.launchservices.secure",
            "LSHandlers",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    // Find the handler block for "https", then the bundle id inside it.
    let https_pos = text.find("\"https\"")?;
    // Enclosing block: search backwards for the previous '{'.
    let start = text[..https_pos].rfind('{')?;
    // End of this block: the next '{' after https_pos, minus 1… the block
    // ends at the matching '}' — approximate with the next block start.
    let block_end = text[https_pos..]
        .find("{")
        .map(|i| https_pos + i)
        .unwrap_or(text.len());
    let block = &text[start..block_end];
    let role_pos = block.find("LSHandlerRoleAll")?;
    let after = &block[role_pos..];
    let quote = after.find('"')?;
    let after_quote = &after[quote + 1..];
    let end = after_quote.find('"')?;
    let bundle = &after_quote[..end];
    match bundle {
        "com.apple.Safari" => Some(BrowserId::Safari),
        "com.google.Chrome" => Some(BrowserId::Chrome),
        "com.microsoft.edgewebkit" => Some(BrowserId::Edge),
        "com.mozilla.firefox" => Some(BrowserId::Firefox),
        "com.brave.Browser" => Some(BrowserId::Brave),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_browsers_on_this_machine() {
        let list = list_browsers();
        for b in &list {
            println!("browser: {} installed={}", b.name, b.installed);
        }
        #[cfg(target_os = "macos")]
        {
            // Safari ships with every macOS install.
            assert!(
                list.iter().any(|b| b.id == "safari" && b.installed),
                "Safari should be detected on macOS"
            );
        }
    }
}

#[cfg(windows)]
fn default_browser_windows() -> Option<BrowserId> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey(
            r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
        )
        .ok()?;
    let prog_id: String = key.get_value("ProgId").ok()?;
    match prog_id.as_str() {
        "ChromeURL" => Some(BrowserId::Chrome),
        "MSEDGE" => Some(BrowserId::Edge),
        "FirefoxURL" => Some(BrowserId::Firefox),
        "BraveURL" => Some(BrowserId::Brave),
        _ => None,
    }
}


#[cfg(test)]
#[test]
fn debug_print_ipc_json() {
    let list = list_browsers();
    let json = serde_json::to_string(&list).unwrap();
    println!("IPC JSON for detect_browsers: {json}");
}
