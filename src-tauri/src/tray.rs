//! System tray (Windows) / menu bar (macOS).
//!
//! Closing the main window hides it to the tray; only "Quit" stops the
//! harness and exits the app.

use crate::process::{ProcessState, StartOutcome};
use crate::state::AppState;
use tauri::{
    menu::{Menu, MenuBuilder, MenuEvent, MenuItemBuilder},
    tray::{TrayIcon, TrayIconBuilder},
    AppHandle, Emitter, Manager,
};

const STATUS_ID: &str = "tray-status";

pub fn build_tray(app: &AppHandle) -> Result<(TrayIcon, Menu<tauri::Wry>), String> {
    let open = MenuItemBuilder::with_id("tray-open", "Open Harness")
        .build(app)
        .map_err(|e| e.to_string())?;
    let restart = MenuItemBuilder::with_id("tray-restart", "Restart")
        .build(app)
        .map_err(|e| e.to_string())?;
    let stop = MenuItemBuilder::with_id("tray-stop", "Stop")
        .build(app)
        .map_err(|e| e.to_string())?;
    let settings = MenuItemBuilder::with_id("tray-settings", "Settings")
        .build(app)
        .map_err(|e| e.to_string())?;
    let quit = MenuItemBuilder::with_id("tray-quit", "Quit")
        .build(app)
        .map_err(|e| e.to_string())?;

    let menu = MenuBuilder::new(app)
        .text(STATUS_ID, "● Stopped")
        .separator()
        .item(&open)
        .item(&restart)
        .item(&stop)
        .separator()
        .item(&settings)
        .separator()
        .item(&quit)
        .build()
        .map_err(|e| e.to_string())?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "no default window icon available".to_string())?;

    let tray = TrayIconBuilder::new()
        .tooltip("DSH Launcher")
        .icon(icon)
        .icon_as_template(false)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu_event(app, event))
        .build(app)
        .map_err(|e| e.to_string())?;

    Ok((tray, menu))
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().0.as_str() {
        "tray-open" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::open_harness_inner(&app).await;
            });
        }
        "tray-restart" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::restart_dsh_inner(&app, Some(true)).await;
            });
        }
        "tray-stop" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::stop_dsh_inner(&app).await;
            });
        }
        "tray-settings" => {
            navigate_to(app, "settings");
        }
        "tray-quit" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::stop_dsh_inner(&app).await;
                app.exit(0);
            });
        }
        _ => {}
    }
}

fn navigate_to(app: &AppHandle, page: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.emit("dsh://navigate", page);
    }
}

/// Refresh the tray's status line.
pub fn update_tray(state: &AppState, menu: &Menu<tauri::Wry>, tray: &TrayIcon) {
    let snap = state.proc.snapshot();
    let status = format!(
        "● {}",
        match snap.state {
            ProcessState::Stopped => "Stopped",
            ProcessState::Starting => "Starting…",
            ProcessState::Running => "Running",
            ProcessState::Stopping => "Stopping…",
            ProcessState::Error => "Error",
        }
    );
    if let Some(kind) = menu.get(STATUS_ID) {
        let _ = kind.as_menuitem_unchecked().set_text(&status);
    }
    let _ = tray.set_tooltip(Some(&format!("DSH Launcher — {}", status.trim_start_matches("● "))));
}

// ---------------------------------------------------------------------------
// Command shims shared by the tray (IPC commands live in commands.rs).
// ---------------------------------------------------------------------------
pub mod commands {
    use super::*;

    pub async fn open_harness_inner(app: &AppHandle) -> Result<(), String> {
        let state = app.state::<AppState>();
        crate::commands::open_harness_impl(&state, app).await
    }

    pub async fn restart_dsh_inner(app: &AppHandle, open_browser: Option<bool>) -> StartOutcome {
        let state = app.state::<AppState>();
        crate::commands::restart_dsh_impl(&state, app, open_browser).await
    }

    pub async fn stop_dsh_inner(app: &AppHandle) -> Result<(), String> {
        let state = app.state::<AppState>();
        crate::commands::stop_dsh_impl(&state, app).await
    }
}
