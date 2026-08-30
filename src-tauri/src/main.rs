// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let launched_by_autostart = std::env::args().any(|a| a == "--autostart");
    dsh_launcher_lib::run(launched_by_autostart);
}
