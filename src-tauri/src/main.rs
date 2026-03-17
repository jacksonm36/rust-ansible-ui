// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "tauri-app")]
fn main() {
    ansible_control_panel::run()
}

#[cfg(not(feature = "tauri-app"))]
fn main() {
    eprintln!("Build the desktop app with default features, or run: cargo run --bin ansible-server --no-default-features --features server-only");
    std::process::exit(1);
}
