//! SQLite database and schema for Ansible UI.

use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

const DEFAULT_DB_PATH: &str = "./data/ansible_ui.db";

/// Get database path from env or default.
fn db_path() -> std::path::PathBuf {
    std::env::var("DATABASE_URL")
        .ok()
        .and_then(|u| {
            u.strip_prefix("sqlite://")
                .or_else(|| u.strip_prefix("sqlite:///"))
                .map(|s| Path::new(s).to_path_buf())
        })
        .unwrap_or_else(|| Path::new(DEFAULT_DB_PATH).to_path_buf())
}

pub fn init_db() -> Result<Connection, rusqlite::Error> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(&path)?;
    create_tables(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn create_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            description TEXT DEFAULT '',
            git_url TEXT,
            git_branch TEXT DEFAULT 'main',
            git_credential_id INTEGER,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS inventories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            description TEXT DEFAULT '',
            content TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS credentials (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'ssh',
            secret_encrypted TEXT,
            extra TEXT DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS job_templates (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            description TEXT DEFAULT '',
            playbook_path TEXT NOT NULL,
            inventory_id INTEGER REFERENCES inventories(id) ON DELETE RESTRICT,
            credential_id INTEGER REFERENCES credentials(id) ON DELETE SET NULL,
            extra_vars TEXT DEFAULT '',
            schedule_enabled INTEGER NOT NULL DEFAULT 0,
            schedule_cron TEXT,
            schedule_tz TEXT DEFAULT 'UTC',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS jobs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            job_template_id INTEGER REFERENCES job_templates(id) ON DELETE SET NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            playbook_path TEXT NOT NULL,
            inventory_content TEXT DEFAULT '',
            extra_vars TEXT DEFAULT '',
            output_log TEXT DEFAULT '',
            started_at TEXT,
            finished_at TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
        CREATE INDEX IF NOT EXISTS idx_inventories_project ON inventories(project_id);
        CREATE INDEX IF NOT EXISTS idx_credentials_project ON credentials(project_id);
        CREATE INDEX IF NOT EXISTS idx_job_templates_project ON job_templates(project_id);
        CREATE INDEX IF NOT EXISTS idx_jobs_project ON jobs(project_id);
        "#,
    )?;
    Ok(())
}

fn migrate(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Add columns if missing (SQLite doesn't have IF NOT EXISTS for columns)
    let migrations: [(&str, &str); 6] = [
        ("projects", "ALTER TABLE projects ADD COLUMN git_url VARCHAR(512)"),
        ("projects", "ALTER TABLE projects ADD COLUMN git_branch VARCHAR(64)"),
        ("projects", "ALTER TABLE projects ADD COLUMN git_credential_id INTEGER"),
        ("job_templates", "ALTER TABLE job_templates ADD COLUMN schedule_enabled INTEGER DEFAULT 0"),
        ("job_templates", "ALTER TABLE job_templates ADD COLUMN schedule_cron VARCHAR(128)"),
        ("job_templates", "ALTER TABLE job_templates ADD COLUMN schedule_tz VARCHAR(64)"),
    ];
    for (table, sql) in migrations {
        if let Err(e) = conn.execute(sql, []) {
            // Ignore "duplicate column" errors
            if !e.to_string().contains("duplicate column") {
                tracing::warn!("Migration {}: {}", table, e);
            }
        }
    }
    Ok(())
}

/// Shared DB handle for the app (used from Axum state). Arc so it can be cloned for threads.
pub type DbPool = std::sync::Arc<Mutex<Connection>>;

pub fn utc_now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_utc_now() {
        let s = utc_now_iso();
        assert!(s.len() >= 20);
    }
}
