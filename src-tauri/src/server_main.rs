//! Standalone server binary. With `embedded-static`, UI is baked in — single file, no `static/` dir needed.

use ansible_control_panel::server::{app, StaticSource};
#[cfg(not(feature = "embedded-static"))]
use std::path::PathBuf;
use std::sync::Arc;

fn resolve_bind() -> String {
    std::env::var("ANSIBLE_UI_BIND").unwrap_or_else(|_| "127.0.0.1:14300".into())
}

fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let static_source = {
        #[cfg(feature = "embedded-static")]
        {
            StaticSource::Embedded
        }
        #[cfg(not(feature = "embedded-static"))]
        {
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
            StaticSource::Filesystem(static_dir)
        }
    };

    let conn = ansible_control_panel::db::init_db().expect("init_db");
    let db = Arc::new(std::sync::Mutex::new(conn));
    ansible_control_panel::scheduler::start_scheduler(db.clone());
    let app = app(static_source, db);

    let bind = resolve_bind();
    eprintln!("Ansible Control Panel listening on http://{}", bind);
    #[cfg(feature = "embedded-static")]
    eprintln!("(embedded UI — no external static/ directory required)");

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
        axum::serve(listener, app).await.expect("serve");
    });
}
