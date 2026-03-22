//! Discover Ansible playbooks and runnable scripts for the job-template picker.

use crate::crud;
use crate::db::DbPool;
use crate::git_support;
use std::fmt;
use walkdir::WalkDir;

#[derive(Debug)]
pub enum PlaybookListError {
    ProjectNotFound,
    Io(String),
}

impl fmt::Display for PlaybookListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlaybookListError::ProjectNotFound => write!(f, "Project not found"),
            PlaybookListError::Io(s) => write!(f, "{s}"),
        }
    }
}

const MAX_DEPTH: usize = 20;
const MAX_FILES: usize = 500;

fn playbook_extensions() -> &'static [&'static str] {
    &["yml", "yaml", "sh", "bash", "ps1", "psm1", "bat", "cmd", "py", "rb"]
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "node_modules" | "vendor" | "__pycache__" | ".venv" | "target" | ".idea"
    )
}

/// Uses the same rules as Git pull summary (`list_playbooks_in_repo`).
pub fn list_playbooks_for_project(db: &DbPool, project_id: i64) -> Result<Vec<String>, PlaybookListError> {
    if crud::get_project(db, project_id).is_none() {
        return Err(PlaybookListError::ProjectNotFound);
    }
    let root = git_support::project_workspace_path(project_id);
    if !root.exists() {
        return Ok(vec![]);
    }
    let root_canon = root
        .canonicalize()
        .map_err(|e| PlaybookListError::Io(e.to_string()))?;
    Ok(git_support::list_playbooks_in_repo(&root_canon))
}

/// Optional paths from the server process working directory (local / non-Git layouts).
pub fn list_playbooks_from_cwd() -> Result<Vec<String>, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root_canon = cwd.canonicalize().map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    let walker = WalkDir::new(&root_canon)
        .max_depth(8.min(MAX_DEPTH))
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_dir() && should_skip_dir(name.as_ref()))
        });

    for entry in walker {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !playbook_extensions().contains(&ext.as_str()) {
            continue;
        }
        let rel = path.strip_prefix(&root_canon).map_err(|_| "invalid path")?;
        let s = rel.to_string_lossy().replace('\\', "/");
        if s.is_empty() || s.contains("..") {
            continue;
        }
        if s.starts_with("workspace/") {
            continue;
        }
        out.push(s);
        if out.len() >= MAX_FILES {
            break;
        }
    }

    out.sort();
    out.dedup();
    Ok(out)
}
