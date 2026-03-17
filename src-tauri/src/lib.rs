#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod crud;
pub mod db;
mod git_support;
mod runner;
mod schemas;
pub mod scheduler;
mod secrets;
pub mod server;

#[allow(dead_code)]
const PORT: u16 = 14300;

#[cfg(feature = "tauri-app")]
use std::path::PathBuf;
#[cfg(feature = "tauri-app")]
use std::sync::Arc;
#[cfg(feature = "tauri-app")]
use tauri::Manager;

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_server_url() -> String {
    format!("http://127.0.0.1:{}", PORT)
}

#[cfg(feature = "tauri-app")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![get_server_url])
        .setup(|app| {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let static_dir = cwd.join("static");
            let static_dir = if static_dir.exists() {
                static_dir
            } else {
                let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
                let root = exe
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .unwrap_or(&exe);
                root.join("static")
            };
            run_server(static_dir, app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(feature = "tauri-app")]
fn run_server(static_dir: PathBuf, _app_handle: tauri::AppHandle) {
    let conn = db::init_db().expect("init_db");
    let db = Arc::new(std::sync::Mutex::new(conn));
    scheduler::start_scheduler(db.clone());
    let app_router = server::app(static_dir, db);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", PORT))
                .await
                .expect("bind");
            axum::serve(listener, app_router).await.expect("serve");
        });
    });
    std::thread::sleep(std::time::Duration::from_millis(500));
}
