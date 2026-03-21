//! Run Ansible playbooks or scripts and capture output.

use crate::crud;
use crate::db::DbPool;
use regex::Regex;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

/// Maximum job output size stored in DB (2 MiB). Larger output is truncated with a notice.
const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const TRUNCATE_SUFFIX: &str = "\n\n[Output truncated: exceeded 2 MB limit]";

fn truncate_output(out: &str) -> String {
    if out.len() <= MAX_OUTPUT_BYTES {
        return out.to_string();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !out.is_char_boundary(end) {
        end -= 1;
    }
    let mut s = String::from_utf8_lossy(out.as_bytes().get(..end).unwrap_or_default()).into_owned();
    s.push_str(TRUNCATE_SUFFIX);
    s
}

/// Ansible picks the inventory plugin from the temp file suffix. YAML content (e.g. `all:` /
/// `hosts:`) must use `.yaml`, or the INI plugin runs and fails with "Invalid host pattern 'all:'".
fn inventory_temp_suffix(content: &str) -> &'static str {
    let s = content.trim_start();
    if s.starts_with('[') {
        return ".ini";
    }
    if s.starts_with("---") {
        return ".yaml";
    }
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            return ".ini";
        }
        // INI host line: `name key=value` or `[group]`
        if t.contains('=') {
            return ".ini";
        }
        if t == "all:" || t == "ungrouped:" || t.starts_with("plugin:") {
            return ".yaml";
        }
        // YAML group header (`webservers:`) before `hosts:` / `children:`
        if t.ends_with(':') {
            return ".yaml";
        }
        break;
    }
    ".ini"
}

/// Allow char in inventory / extra-vars text (drop invisibles that break OpenSSH host parsing).
fn ansible_text_char_ok(c: char) -> bool {
    match c {
        '\0' => false,
        // CRLF handled before this pass; drop other ASCII controls except newline/tab (YAML indent)
        c if c.is_ascii_control() && c != '\n' && c != '\t' => false,
        '\u{00ad}' | '\u{034f}' | '\u{061c}' => false, // soft hyphen, CGJ, ALM
        '\u{115f}' | '\u{1160}' | '\u{17b4}' | '\u{17b5}' | '\u{180e}' => false,
        '\u{200b}'..='\u{200f}' => false, // ZWSP, ZWNJ, ZWJ, marks
        '\u{202a}'..='\u{202e}' => false, // bidi embedding overrides
        '\u{2060}'..='\u{2064}' => false, // word joiner, invisible ops
        '\u{2066}'..='\u{2069}' => false, // isolate pops
        '\u{feff}' => false,
        '\u{fff0}'..='\u{fff8}' => false,
        _ => true,
    }
}

/// CRLF/BOM/NUL, trailing spaces, and invisible Unicode (common after Windows/Web copy-paste) break OpenSSH hostnames.
fn normalize_ansible_text(s: &str) -> String {
    let s = s.trim_start_matches('\u{feff}');
    let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .filter(|&c| ansible_text_char_ok(c))
        .collect()
}

fn sanitize_inventory_content(s: &str) -> String {
    let mut out = normalize_ansible_text(s);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// YAML single-quoted scalar (`'` → `''`). Safe for passwords (no `$` / escape surprises).
fn yaml_single_quoted_scalar(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Extra vars from SSH password + optional credential `extra` (e.g. `ansible_user: root`).
fn build_credential_extra_vars_yaml(ssh_password: Option<&str>, credential_extra: &str) -> Option<String> {
    let extra = normalize_ansible_text(credential_extra.trim());
    let mut body = String::new();
    if let Some(pass) = ssh_password {
        let q = yaml_single_quoted_scalar(pass.trim());
        body.push_str("ansible_ssh_pass: ");
        body.push_str(&q);
        body.push('\n');
        body.push_str("ansible_password: ");
        body.push_str(&q);
        body.push('\n');
    }
    if !extra.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str(&extra);
        if !body.ends_with('\n') {
            body.push('\n');
        }
    }
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

const SCRIPT_EXTENSIONS: &[(&str, &[&str])] = &[
    (".sh", &["bash"]),
    (".bash", &["bash"]),
    (".ps1", &["powershell", "-ExecutionPolicy", "Bypass", "-File"]),
    (".psm1", &["powershell", "-ExecutionPolicy", "Bypass", "-File"]),
    (".bat", &["cmd", "/c"]),
    (".cmd", &["cmd", "/c"]),
    (".py", &["python3"]),
    (".rb", &["ruby"]),
];

lazy_static::lazy_static! {
    static ref ENV_VAR_RE: Regex = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
}

fn parse_timeout_secs(var: Option<String>, default: u64) -> u64 {
    var.and_then(|s| s.parse().ok())
        .filter(|&n| n > 0 && n <= 604_800)
        .unwrap_or(default)
}

fn playbook_timeout_secs() -> u64 {
    let primary = std::env::var("ANSIBLE_UI_PLAYBOOK_TIMEOUT_SECS").ok();
    let fallback = std::env::var("ANSIBLE_UI_JOB_TIMEOUT_SECS").ok();
    parse_timeout_secs(primary.or(fallback), 3600)
}

fn script_timeout_secs() -> u64 {
    parse_timeout_secs(std::env::var("ANSIBLE_UI_SCRIPT_TIMEOUT_SECS").ok(), 3600)
}

fn is_script(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    SCRIPT_EXTENSIONS.iter().any(|(e, _)| *e == ext)
}

enum CapturedRun {
    Finished(std::process::ExitStatus, Vec<u8>, Vec<u8>),
    TimedOut(Vec<u8>, Vec<u8>),
    IoError(String),
}

fn run_command_capturing(mut cmd: Command, timeout_secs: u64) -> CapturedRun {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return CapturedRun::IoError(e.to_string()),
    };
    let mut out_pipe = match child.stdout.take() {
        Some(o) => o,
        None => return CapturedRun::IoError("stdout not piped".into()),
    };
    let mut err_pipe = match child.stderr.take() {
        Some(e) => e,
        None => return CapturedRun::IoError("stderr not piped".into()),
    };
    let (tx_o, rx_o) = mpsc::channel();
    let (tx_e, rx_e) = mpsc::channel();
    thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out_pipe.read_to_end(&mut v);
        let _ = tx_o.send(v);
    });
    thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err_pipe.read_to_end(&mut v);
        let _ = tx_e.send(v);
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = rx_o.recv().unwrap_or_default();
            let stderr = rx_e.recv().unwrap_or_default();
            return CapturedRun::TimedOut(stdout, stderr);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = rx_o.recv().unwrap_or_default();
                let stderr = rx_e.recv().unwrap_or_default();
                return CapturedRun::Finished(status, stdout, stderr);
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => return CapturedRun::IoError(e.to_string()),
        }
    }
}

fn run_script(script_path: &str, extra_vars: &str, timeout_secs: u64) -> (i32, String) {
    let ext = Path::new(script_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let runner = SCRIPT_EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, r)| r.to_vec());
    let runner = match runner {
        Some(r) => r,
        None => return (1, format!("No runner for extension {}", ext)),
    };

    let abs_path =
        std::fs::canonicalize(script_path).unwrap_or_else(|_| Path::new(script_path).to_path_buf());
    if !abs_path.is_file() {
        return (1, format!("Script not found: {}", abs_path.display()));
    }

    let cwd = abs_path.parent().unwrap_or(Path::new("."));
    let mut cmd = Command::new(runner[0]);
    if runner.len() > 1 {
        cmd.args(&runner[1..]);
    }
    cmd.arg(&abs_path);
    cmd.current_dir(cwd);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let extra_vars = normalize_ansible_text(extra_vars.trim());
    for line in extra_vars.lines() {
        let line = line.trim();
        if line.contains('=') && !line.starts_with('#') {
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                if !k.is_empty() && ENV_VAR_RE.is_match(k) {
                    cmd.env(k, v);
                }
            }
        }
    }

    match run_command_capturing(cmd, timeout_secs) {
        CapturedRun::Finished(status, stdout, stderr) => {
            let out = String::from_utf8_lossy(&stdout).to_string()
                + &String::from_utf8_lossy(&stderr);
            (status.code().unwrap_or(1), truncate_output(&out))
        }
        CapturedRun::TimedOut(stdout, stderr) => {
            let mut out = String::from_utf8_lossy(&stdout).to_string()
                + &String::from_utf8_lossy(&stderr);
            out.push_str(&format!(
                "\n\n[Process killed: exceeded {}s timeout]",
                timeout_secs
            ));
            (124, truncate_output(&out))
        }
        CapturedRun::IoError(e) => (1, e),
    }
}

/// Run playbook (or script) and update job status in DB.
pub fn run_playbook(
    db: &DbPool,
    job_id: i64,
    playbook_path: &str,
    inventory_content: &str,
    extra_vars: &str,
    credential_ssh_key: Option<&str>,
    credential_ssh_password: Option<&str>,
    credential_vault_password: Option<&str>,
    credential_extra: &str,
) -> (String, String) {
    let _ = crud::update_job_status(db, job_id, "running", "");

    if is_script(playbook_path) {
        let to = script_timeout_secs();
        let (code, out) = run_script(playbook_path, extra_vars, to);
        let status = if code == 0 { "success" } else { "failed" };
        let out_capped = truncate_output(&out);
        let _ = crud::update_job_status(db, job_id, status, &out_capped);
        return (status.to_string(), out);
    }

    let playbook_abs = std::fs::canonicalize(playbook_path)
        .unwrap_or_else(|_| Path::new(playbook_path).to_path_buf());
    if !playbook_abs.is_file() {
        let msg = format!("Playbook file not found: {}", playbook_abs.display());
        let _ = crud::update_job_status(db, job_id, "failed", &msg);
        return ("failed".into(), msg);
    }

    let inv_content = if inventory_content.is_empty() {
        "[all]\nlocalhost ansible_connection=local\n".to_string()
    } else {
        sanitize_inventory_content(inventory_content)
    };

    let inv_suffix = inventory_temp_suffix(&inv_content);
    let inv = match NamedTempFile::with_suffix(inv_suffix) {
        Ok(f) => f,
        Err(e) => {
            let msg = format!("Failed to create temporary inventory file: {}", e);
            let _ = crud::update_job_status(db, job_id, "failed", &msg);
            return ("failed".to_string(), msg);
        }
    };
    let _ = std::fs::write(inv.path(), &inv_content);

    let mut _key_file = None;
    let mut _vault_file = None;
    let mut _extra_vars_file = None;

    let mut args: Vec<String> = vec![
        playbook_abs.to_string_lossy().to_string(),
        "-i".into(),
        inv.path().to_string_lossy().to_string(),
    ];

    // Credential vars first; job-template extra_vars second (overrides on duplicate keys).
    if let Some(yaml) = build_credential_extra_vars_yaml(credential_ssh_password, credential_extra) {
        if let Ok(f) = NamedTempFile::with_suffix(".yml") {
            if std::fs::write(f.path(), yaml).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).ok();
                }
                let path_str = format!("@{}", f.path().display());
                _extra_vars_file = Some(f);
                args.push("-e".into());
                args.push(path_str);
            }
        }
    }

    let extra_norm = normalize_ansible_text(extra_vars.trim());
    if !extra_norm.is_empty() {
        args.push("-e".into());
        args.push(extra_norm);
    }

    if let Some(key) = credential_ssh_key {
        if let Ok(f) = NamedTempFile::with_suffix(".pem") {
            let key = key.trim();
            let content = if key.ends_with('\n') {
                key.to_string()
            } else {
                format!("{}\n", key)
            };
            if std::fs::write(f.path(), content).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).ok();
                }
                let path_str = f.path().to_string_lossy().to_string();
                _key_file = Some(f);
                args.push("--private-key".into());
                args.push(path_str);
            }
        }
    }

    if let Some(vault) = credential_vault_password {
        if let Ok(f) = NamedTempFile::with_suffix(".vault") {
            if std::fs::write(f.path(), format!("{}\n", vault.trim())).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600)).ok();
                }
                let path_str = f.path().to_string_lossy().to_string();
                _vault_file = Some(f);
                args.push("--vault-password-file".into());
                args.push(path_str);
            }
        }
    }

    let cwd = playbook_abs.parent().unwrap_or(Path::new("."));
    let mut cmd = Command::new("ansible-playbook");
    cmd.args(&args).current_dir(cwd);
    if let Ok(u) = std::env::var("ANSIBLE_UI_REMOTE_USER") {
        let u = u.trim();
        if !u.is_empty() {
            // Default remote user when inventory omits ansible_user (systemd runs as ansible-ui).
            cmd.env("ANSIBLE_REMOTE_USER", u);
        }
    }
    let host_key_check =
        std::env::var("ANSIBLE_HOST_KEY_CHECKING").unwrap_or_else(|_| "False".into());
    cmd.env("ANSIBLE_HOST_KEY_CHECKING", &host_key_check);
    cmd.env("PYTHONIOENCODING", "utf-8");
    cmd.env("PYTHONUTF8", "1");
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let timeout_secs = playbook_timeout_secs();
    let outcome = run_command_capturing(cmd, timeout_secs);

    let (out, status_str) = match outcome {
        CapturedRun::Finished(status, stdout, stderr) => {
            let out = String::from_utf8_lossy(&stdout).to_string()
                + &String::from_utf8_lossy(&stderr);
            let s = if status.success() { "success" } else { "failed" };
            (out, s)
        }
        CapturedRun::TimedOut(stdout, stderr) => {
            let mut out = String::from_utf8_lossy(&stdout).to_string()
                + &String::from_utf8_lossy(&stderr);
            out.push_str(&format!(
                "\n\n[ansible-playbook killed: exceeded {}s timeout]",
                timeout_secs
            ));
            (out, "failed")
        }
        CapturedRun::IoError(e) => {
            let _ = crud::update_job_status(db, job_id, "failed", &e);
            return ("failed".into(), e);
        }
    };

    let out_capped = truncate_output(&out);
    let _ = crud::update_job_status(db, job_id, status_str, &out_capped);
    (status_str.to_string(), out)
}
