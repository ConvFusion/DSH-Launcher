//! Opening URLs in a chosen browser.

use super::detector::BrowserId;
use crate::config::log;
use std::process::Command;

/// Open `url` in the OS default browser (used when the user hasn't picked
/// one yet — better than silently doing nothing).
pub fn open_url_default(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("failed to open default browser: {e}"))?;
        log(&format!("opened {url} in the default browser"));
        Ok(())
    }

    #[cfg(windows)]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map_err(|e| format!("failed to open default browser: {e}"))?;
        log(&format!("opened {url} in the default browser"));
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // Linux & co: hand the URL to xdg-open (the freedesktop standard).
        Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("failed to open default browser (xdg-open): {e}"))?;
        log(&format!("opened {url} in the default browser"));
        Ok(())
    }
}

/// Open `url` in `browser`. Never opens a browser that isn't installed.
pub fn open_url(browser: BrowserId, url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app_name: &str = match browser {
            BrowserId::Chrome => "Google Chrome",
            BrowserId::Edge => "Microsoft Edge",
            BrowserId::Firefox => "Firefox",
            BrowserId::Brave => "Brave Browser",
            BrowserId::Safari => "Safari",
        };
        Command::new("open")
            .arg("-a")
            .arg(app_name)
            .arg(url)
            .spawn()
            .map_err(|e| format!("failed to open {app_name}: {e}"))?;
        log(&format!("opened {url} in {app_name}"));
        Ok(())
    }

    #[cfg(windows)]
    {
        let exe = super::detector::installed_path(browser)
            .ok_or_else(|| format!("{} is not installed on this computer.", browser.display_name()))?;
        Command::new(&exe)
            .arg("--new-window")
            .arg(url)
            .spawn()
            .map_err(|e| format!("failed to start {}: {e}", browser.display_name()))?;
        log(&format!("opened {url} in {}", browser.display_name()));
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        // Linux & co: launch the detected browser executable with the URL.
        let exe = super::detector::installed_path(browser)
            .ok_or_else(|| format!("{} is not installed on this computer.", browser.display_name()))?;
        Command::new(&exe)
            .arg(url)
            .spawn()
            .map_err(|e| format!("failed to start {}: {e}", browser.display_name()))?;
        log(&format!("opened {url} in {}", browser.display_name()));
        Ok(())
    }
}
