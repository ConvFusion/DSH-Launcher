mod browser;
mod commands;
mod config;
mod process;
mod runtime;
mod state;
mod tray;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(launched_by_autostart: bool) {
    let cfg = config::Config::load();
    config::log(&format!(
        "dsh-launcher v{} starting (autostart={launched_by_autostart})",
        env!("CARGO_PKG_VERSION")
    ));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // A second double-click just focuses the existing window.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new(cfg, launched_by_autostart))
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Closing the window hides the launcher to the tray;
                // the DSH service keeps running.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let (tray_icon, tray_menu) = tray::build_tray(&handle)?;
            if let Some(state) = app.try_state::<AppState>() {
                *state.tray.lock().unwrap() = Some((tray_icon, tray_menu));
            }
            let startup_handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                state::startup(startup_handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::ensure_environment,
            commands::install_node_runtime,
            commands::install_dsh_package,
            commands::install_dsh_plugin,
            commands::check_dsh_update,
            commands::start_dsh,
            commands::stop_dsh,
            commands::restart_dsh,
            commands::open_harness,
            commands::detect_browsers,
            commands::select_browser,
            commands::write_debug,
            commands::get_config,
            commands::update_config,
            commands::set_autostart,
            commands::read_log,
            commands::open_log_dir,
            commands::quit_app,
            commands::suggest_ports,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // macOS: clicking the Dock icon reopens the app even when the
            // window is hidden to the tray — show the main window again.
            // (RunEvent::Reopen only exists on macOS.)
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app_handle.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app_handle, event);
            }
        });
}
