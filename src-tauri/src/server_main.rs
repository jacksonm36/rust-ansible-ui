//! Standalone server binary. Run this first, then `cargo tauri dev` in another terminal.

use std::path::PathBuf;
use std::sync::Arc;

const PORT: u16 = 14300;

fn main() {
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
            .unwrap_or(&cwd);
        root.join("static")
    };

    let conn = ansible_control_panel::db::init_db().expect("init_db");
    let db = Arc::new(std::sync::Mutex::new(conn));
    ansible_control_panel::scheduler::start_scheduler(db.clone());
    let app = ansible_control_panel::server::app(static_dir, db);

    println!("Ansible Control Panel server at http://127.0.0.1:{}", PORT);
    println!("Run 'cargo tauri dev' in another terminal to open the app window.");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", PORT))
            .await
            .expect("bind");
        axum::serve(listener, app).await.expect("serve");
    });
}
