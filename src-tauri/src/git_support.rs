//! Git clone/pull for project repositories.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use tempfile::NamedTempFile;

static WORKSPACE_LOCK: Mutex<()> = Mutex::new(());

fn workspace_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let ws = cwd.join("workspace");
    if ws.exists() || cwd.join("static").exists() {
        return ws;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let root = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).and_then(|p| p.parent()).unwrap_or(&cwd);
    root.join("workspace")
}

fn workspace_path(project_id: i64) -> PathBuf {
    let _guard = WORKSPACE_LOCK.lock().unwrap();
    let ws = workspace_dir();
    std::fs::create_dir_all(&ws).ok();
    ws.join(format!("project_{}", project_id))
}

lazy_static::lazy_static! {
    static ref BRANCH_RE: Regex = Regex::new(r"^[a-zA-Z0-9._\-/]+$").unwrap();
}

pub fn validate_branch(branch: &str) -> Result<String, String> {
    let b = branch.trim();
    let b = if b.is_empty() { "main" } else { b };
    if !BRANCH_RE.is_match(b) {
        return Err("Invalid branch name. Only alphanumeric characters, hyphens, underscores, dots, and slashes are allowed.".into());
    }
    if b.starts_with('-') || b.starts_with("..") || b.contains("..") {
        return Err("Invalid branch name.".into());
    }
    Ok(b.to_string())
}

fn is_ssh_url(url: &str) -> bool {
    url.trim().starts_with("git@") || url.split("://").next().map(|s| s.contains("git@")).unwrap_or(false)
}

pub fn normalize_git_url(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() {
        return u.to_string();
    }
    // GitHub: .../owner/repo/blob/branch/... or .../owner/repo/tree/branch/...
    if let Some(caps) = Regex::new(r"(?i)^(https?://(?:www\.)?github\.com/[^/]+/[^/]+?)(?:/blob/[^/]+/.*|/tree/[^/]+/.*)?/?$").ok().and_then(|re| re.captures(u)) {
        let base = caps.get(1).map(|m| m.as_str().trim_end_matches('/')).unwrap_or(u);
        return if base.ends_with(".git") { base.to_string() } else { format!("{}.git", base) };
    }
    // GitLab
    if let Some(caps) = Regex::new(r"(?i)^(https?://[^/]+/[^/]+/[^/]+?)(?:/-/blob/.*|/-/tree/.*)?/?$").ok().and_then(|re| re.captures(u)) {
        let base = caps.get(1).map(|m| m.as_str().trim_end_matches('/')).unwrap_or(u);
        return if base.ends_with(".git") { base.to_string() } else { format!("{}.git", base) };
    }
    u.to_string()
}

pub fn clone_or_pull(
    project_id: i64,
    git_url: &str,
    branch: &str,
    ssh_private_key: Option<&str>,
    https_token: Option<&str>,
) -> Result<PathBuf, String> {
    let url = normalize_git_url(git_url);
    if url.is_empty() {
        return Err("git_url is required".into());
    }
    let branch = validate_branch(branch)?;
    let repo_path = workspace_path(project_id);

    let mut _key_file: Option<NamedTempFile> = None;
    let mut _creds_file: Option<NamedTempFile> = None;
    let mut envs: Vec<(String, String)> = std::env::vars().collect();

    if is_ssh_url(&url) {
        if let Some(key) = ssh_private_key {
            let key = key.trim();
            let mut f = NamedTempFile::new().map_err(|e| e.to_string())?;
            use std::io::Write;
            f.write_all(key.as_bytes()).map_err(|e| e.to_string())?;
            if !key.ends_with('\n') {
                f.write_all(b"\n").map_err(|e| e.to_string())?;
            }
            f.flush().map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).ok();
            }
            let path = f.path().to_path_buf();
            envs.push(("GIT_SSH_COMMAND".into(), format!("ssh -i \"{}\" -o StrictHostKeyChecking=accept-new", path.display())));
            _key_file = Some(f);
        }
    } else if let Some(token) = https_token {
        let cred_entry = format!("{}://x-access-token:{}@{}",
            url.split("://").next().unwrap_or("https"),
            token,
            url.replace("https://", "").replace("http://", "").split('/').next().unwrap_or("")
        );
        let mut f = NamedTempFile::new().map_err(|e| e.to_string())?;
        use std::io::Write;
        f.write_all(cred_entry.as_bytes()).map_err(|e| e.to_string())?;
        f.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).ok();
        }
        let path = f.path().to_path_buf();
        envs.push(("GIT_CONFIG_COUNT".into(), "2".into()));
        envs.push(("GIT_CONFIG_KEY_0".into(), "credential.helper".into()));
        envs.push(("GIT_CONFIG_VALUE_0".into(), "".into()));
        envs.push(("GIT_CONFIG_KEY_1".into(), "credential.helper".into()));
        envs.push(("GIT_CONFIG_VALUE_1".into(), format!("store --file={}", path.display())));
        envs.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
        _creds_file = Some(f);
    }

    let run = |cmd: &mut Command| {
        for (k, v) in &envs {
            cmd.env(k, v);
        }
        let out = cmd.output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(stderr.to_string());
        }
        Ok(())
    };

    if repo_path.join(".git").exists() {
        run(Command::new("git").args(["fetch", "origin", &branch]).current_dir(&repo_path))?;
        run(Command::new("git").args(["checkout", &branch]).current_dir(&repo_path))?;
        {
            let mut cmd = Command::new("git");
            cmd.args(["pull", "origin", &branch]).current_dir(&repo_path);
            for (k, v) in &envs { cmd.env(k, v); }
            run(&mut cmd)?;
        }
    } else {
        if repo_path.exists() {
            std::fs::remove_dir_all(&repo_path).map_err(|e| e.to_string())?;
        }
        {
            let repo_str = repo_path.to_string_lossy().into_owned();
            let mut cmd = Command::new("git");
            cmd.args(["clone", "--branch", &branch, "--single-branch", "--depth", "50", &url, &repo_str]);
            for (k, v) in &envs { cmd.env(k, v); }
            run(&mut cmd)?;
        }
    }

    Ok(repo_path)
}

const WALK_SUFFIXES: &[&str] = &[
    ".yml", ".yaml", ".sh", ".bash", ".zsh", ".csh", ".ksh", ".ps1", ".psm1", ".bat", ".cmd",
    ".tf", ".tfvars", ".hcl", ".py", ".rb",
];

fn should_skip_path(rel: &Path) -> bool {
    rel.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.starts_with('.') || s == "group_vars" || s == "host_vars"
    })
}

pub fn list_playbooks_in_repo(repo_path: &Path) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut playbooks = Vec::new();

    for entry in walkdir::WalkDir::new(repo_path)
        .into_iter()
        .filter_entry(|e| !e.path().components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) && e.file_name().to_string_lossy() != "group_vars" && e.file_name().to_string_lossy() != "host_vars")
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let rel = match path.strip_prefix(repo_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if should_skip_path(rel) {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if WALK_SUFFIXES.contains(&ext.as_str()) {
            let key = rel.to_string_lossy().replace('\\', "/");
            if seen.insert(key.clone()) {
                playbooks.push(key);
            }
        }
    }

    playbooks.sort_by(|a, b| {
        let a_ansible = a.to_lowercase().ends_with(".yml") || a.to_lowercase().ends_with(".yaml");
        let b_ansible = b.to_lowercase().ends_with(".yml") || b.to_lowercase().ends_with(".yaml");
        match (a_ansible, b_ansible) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.cmp(b),
        }
    });
    playbooks
}
