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
///
/// **Do not** treat a leading `---` alone as YAML: documents like `---\n192.168.1.247` parse as a
/// root scalar and break the yaml inventory plugin ("expected dictionary and got: …").
fn inventory_temp_suffix(content: &str) -> &'static str {
    let s = content.trim_start();
    if s.starts_with('[') {
        return ".ini";
    }
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "---" || t == "..." {
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

/// True if `s` is a dotted IPv4 (optional quotes), nothing else.
fn token_is_ipv4_host(s: &str) -> bool {
    let s = s.trim().trim_matches(|c| c == '"' || c == '\'');
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    for p in parts {
        if p.is_empty() || p.len() > 3 || !p.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if p.parse::<u8>().is_err() {
            return false;
        }
    }
    true
}

/// One address per line, optional `---` / `…` markers, optional comments → Ansible INI `[scanned]`.
/// Prevents `.yaml` temp files whose YAML root is a bare string (Ansible: "expected dictionary").
fn rewrite_bare_ip_lines_to_ini(s: &str) -> Option<String> {
    let mut hosts: Vec<&str> = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "---" || t == "..." {
            continue;
        }
        if !token_is_ipv4_host(t) {
            return None;
        }
        hosts.push(t.trim_matches(|c| c == '"' || c == '\''));
    }
    if hosts.is_empty() {
        return None;
    }
    let mut ini = String::from("[scanned]\n");
    for ip in hosts {
        ini.push_str(ip);
        ini.push('\n');
    }
    Some(ini)
}

/// Under `hosts:` each entry must be a **host name** (key), then vars like `ansible_host:` beneath it.
/// Fix common mistake: `hosts:` immediately followed by `ansible_host:` (no host key).
fn fix_hosts_direct_ansible_host(content: &str) -> String {
    let lines: Vec<String> = content.lines().map(String::from).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "hosts:" && i + 1 < lines.len() {
            let trimmed_next = lines[i + 1].trim();
            if let Some(rest) = trimmed_next.strip_prefix("ansible_host:") {
                let ip = rest
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'');
                if token_is_ipv4_host(ip) {
                    out.push(lines[i].clone());
                    let hosts_indent = lines[i].chars().take_while(|c| *c == ' ').count();
                    out.push(format!(
                        "{}\"{}\":",
                        " ".repeat(hosts_indent.saturating_add(2)),
                        ip
                    ));
                    out.push(lines[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
        }
        out.push(lines[i].clone());
        i += 1;
    }
    out.join("\n")
}

fn sanitize_inventory_content(s: &str) -> String {
    let mut out = normalize_ansible_text(s);
    for _ in 0..16 {
        let fixed = fix_hosts_direct_ansible_host(&out);
        if fixed == out {
            break;
        }
        out = fixed;
    }
    if let Some(ini) = rewrite_bare_ip_lines_to_ini(&out) {
        out = ini;
    }
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

fn run_command_with_live_updates(
    db: &DbPool,
    job_id: i64,
    mut cmd: Command,
    timeout_secs: u64,
) -> CapturedRun {
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

    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    let tx_o = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match out_pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = tx_o.send((true, buf[..n].to_vec()));
                }
                Err(_) => break,
            }
        }
    });
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match err_pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = tx.send((false, buf[..n].to_vec()));
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut stdout = Vec::<u8>::new();
    let mut stderr = Vec::<u8>::new();
    let mut merged = String::new();
    let mut last_flush = Instant::now();
    let flush_every = Duration::from_millis(700);

    loop {
        while let Ok((is_stdout, chunk)) = rx.try_recv() {
            if is_stdout {
                stdout.extend_from_slice(&chunk);
            } else {
                stderr.extend_from_slice(&chunk);
            }
            merged.push_str(&String::from_utf8_lossy(&chunk));
        }

        if last_flush.elapsed() >= flush_every {
            let _ = crud::update_job_status(db, job_id, "running", &truncate_output(&merged));
            last_flush = Instant::now();
        }

        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            while let Ok((is_stdout, chunk)) = rx.try_recv() {
                if is_stdout {
                    stdout.extend_from_slice(&chunk);
                } else {
                    stderr.extend_from_slice(&chunk);
                }
            }
            return CapturedRun::TimedOut(stdout, stderr);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                while let Ok((is_stdout, chunk)) = rx.try_recv() {
                    if is_stdout {
                        stdout.extend_from_slice(&chunk);
                    } else {
                        stderr.extend_from_slice(&chunk);
                    }
                }
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

/// Arguments for [`run_playbook`].
pub struct PlaybookRunParams<'a> {
    pub db: &'a DbPool,
    pub job_id: i64,
    pub playbook_path: &'a str,
    pub inventory_content: &'a str,
    pub extra_vars: &'a str,
    pub ssh_key: Option<&'a str>,
    pub ssh_password: Option<&'a str>,
    pub vault_password: Option<&'a str>,
    pub credential_extra: &'a str,
}

/// Run playbook (or script) and update job status in DB.
pub fn run_playbook(p: PlaybookRunParams<'_>) -> (String, String) {
    let _ = crud::update_job_status(p.db, p.job_id, "running", "");

    if is_script(p.playbook_path) {
        let to = script_timeout_secs();
        let (code, out) = run_script(p.playbook_path, p.extra_vars, to);
        let status = if code == 0 { "success" } else { "failed" };
        let out_capped = truncate_output(&out);
        let _ = crud::update_job_status(p.db, p.job_id, status, &out_capped);
        return (status.to_string(), out);
    }

    let playbook_abs = std::fs::canonicalize(p.playbook_path)
        .unwrap_or_else(|_| Path::new(p.playbook_path).to_path_buf());
    if !playbook_abs.is_file() {
        let msg = format!("Playbook file not found: {}", playbook_abs.display());
        let _ = crud::update_job_status(p.db, p.job_id, "failed", &msg);
        return ("failed".into(), msg);
    }

    let inv_content = if p.inventory_content.is_empty() {
        "[all]\nlocalhost ansible_connection=local\n".to_string()
    } else {
        sanitize_inventory_content(p.inventory_content)
    };

    let inv_suffix = inventory_temp_suffix(&inv_content);
    let inv = match NamedTempFile::with_suffix(inv_suffix) {
        Ok(f) => f,
        Err(e) => {
            let msg = format!("Failed to create temporary inventory file: {}", e);
            let _ = crud::update_job_status(p.db, p.job_id, "failed", &msg);
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
    if let Some(yaml) = build_credential_extra_vars_yaml(p.ssh_password, p.credential_extra) {
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

    let extra_norm = normalize_ansible_text(p.extra_vars.trim());
    if !extra_norm.is_empty() {
        args.push("-e".into());
        args.push(extra_norm);
    }

    if let Some(key) = p.ssh_key {
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

    if let Some(vault) = p.vault_password {
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
    let outcome = run_command_with_live_updates(p.db, p.job_id, cmd, timeout_secs);

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
            let _ = crud::update_job_status(p.db, p.job_id, "failed", &e);
            return ("failed".into(), e);
        }
    };

    let out_capped = truncate_output(&out);
    let _ = crud::update_job_status(p.db, p.job_id, status_str, &out_capped);
    (status_str.to_string(), out)
}
