//! LAN ping sweep (ICMP via system `ping`) and SSH public key extraction (`ssh-keygen -y`).

use crate::crud;
use crate::db::DbPool;
use std::io::Write;
use std::net::Ipv4Addr;
#[cfg(unix)]
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::time::Duration;
use tempfile::NamedTempFile;

const MAX_HOSTS: u32 = 1024;

#[derive(Debug, serde::Serialize)]
pub struct ScanHost {
    pub ip: String,
    pub alive: bool,
}

/// Parse `a.b.c.d/prefix` (IPv4 only). Host count must be ≤ 1024.
pub fn parse_ipv4_cidr(cidr: &str) -> Result<(u32, u8), String> {
    let s = cidr.trim();
    if s.is_empty() {
        return Err("CIDR is empty".into());
    }
    if s.len() > 48 {
        return Err("CIDR string is too long".into());
    }
    let (addr_s, prefix_s) = s.split_once('/').ok_or_else(|| {
        "Invalid CIDR. Use IPv4 like 192.168.1.0/24".to_string()
    })?;
    let prefix: u8 = prefix_s
        .trim()
        .parse()
        .map_err(|_| "Invalid prefix length".to_string())?;
    if !(8..=32).contains(&prefix) {
        return Err("Prefix must be between /8 and /32".into());
    }
    let host_bits = 32u32.saturating_sub(u32::from(prefix));
    let num_hosts = 1u64.checked_shl(host_bits).ok_or("CIDR too large")?;
    if num_hosts > u64::from(MAX_HOSTS) {
        return Err(format!(
            "Subnet too large ({} addresses). Maximum {} hosts per scan.",
            num_hosts, MAX_HOSTS
        ));
    }
    let addr: Ipv4Addr = addr_s
        .trim()
        .parse()
        .map_err(|_| "Invalid IPv4 address".to_string())?;
    let base = u32::from(addr);
    let mask = !((1u64 << (32 - u32::from(prefix))) - 1) as u32;
    let network = base & mask;
    Ok((network, prefix))
}

pub fn cidr_to_ips(network: u32, prefix: u8) -> Vec<String> {
    let host_bits = 32u32.saturating_sub(u32::from(prefix));
    let n = 1u32 << host_bits;
    (0..n)
        .map(|i| {
            let ip = network | i;
            Ipv4Addr::from(ip).to_string()
        })
        .collect()
}

/// True if stdout looks like a normal ICMP echo reply (exit code is not always reliable).
#[cfg(unix)]
fn ping_stdout_suggests_reply(stdout: &[u8]) -> bool {
    let s = String::from_utf8_lossy(stdout);
    // GNU/iputils: "64 bytes from 192.168.1.1: icmp_seq=1 ttl=64 time=0.3 ms"
    s.contains("bytes from") && s.contains("icmp_seq")
}

#[cfg(target_os = "linux")]
fn ping_icmp_linux(ip: &str) -> bool {
    // Systemd services often have a minimal PATH — prefer absolute paths.
    // Unprivileged ICMP on Linux uses unprivileged ping socket; `-4` avoids IPv6 quirks.
    let bins = ["/bin/ping", "/usr/bin/ping", "ping"];
    let flag_sets: &[&[&str]] = &[
        &["-4", "-n", "-c", "1", "-W", "3"],
        &["-c", "1", "-W", "3"],
        // BusyBox / some minimal systems (deadline seconds)
        &["-c", "1", "-w", "3"],
    ];
    for bin in bins {
        for flags in flag_sets {
            let mut cmd = Command::new(bin);
            cmd.args(flags.iter().copied());
            cmd.arg(ip);
            let Ok(out) = cmd.output() else {
                continue;
            };
            if out.status.success() || ping_stdout_suggests_reply(&out.stdout) {
                return true;
            }
        }
    }
    false
}

#[cfg(all(unix, not(target_os = "linux")))]
fn ping_icmp_unix(ip: &str) -> bool {
    let bins = ["/sbin/ping", "/bin/ping", "ping"];
    let attempts: &[&[&str]] = &[
        &["-c", "1", "-W", "2000"],
        &["-c", "1", "-t", "2"],
        &["-c", "1", "-W", "2"],
    ];
    for bin in bins {
        for flags in attempts {
            let mut cmd = Command::new(bin);
            cmd.args(flags.iter().copied());
            cmd.arg(ip);
            let Ok(out) = cmd.output() else {
                continue;
            };
            if out.status.success() || ping_stdout_suggests_reply(&out.stdout) {
                return true;
            }
        }
    }
    false
}

/// When ICMP is blocked or the service user cannot use ping, TCP connect still shows "something is there"
/// (typical for SSH deployer: port 22).
#[cfg(unix)]
fn tcp_probe_host(ip: &str) -> bool {
    let Ok(addr) = ip.parse::<Ipv4Addr>() else {
        return false;
    };
    let timeout = Duration::from_millis(450);
    for port in [22u16, 80, 443, 445, 139] {
        let sock = SocketAddr::from((addr, port));
        if TcpStream::connect_timeout(&sock, timeout).is_ok() {
            return true;
        }
    }
    false
}

fn ping_host(ip: &str) -> bool {
    #[cfg(windows)]
    {
        match Command::new("ping")
            .args(["-n", "1", "-w", "750", ip])
            .output()
        {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(target_os = "linux")]
    {
        ping_icmp_linux(ip) || tcp_probe_host(ip)
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        ping_icmp_unix(ip) || tcp_probe_host(ip)
    }
}

pub fn scan_cidr(cidr: &str) -> Result<Vec<ScanHost>, String> {
    let (network, prefix) = parse_ipv4_cidr(cidr)?;
    let ips = cidr_to_ips(network, prefix);
    use std::thread;

    const CHUNK: usize = 48;
    let results = std::sync::Mutex::new(Vec::with_capacity(ips.len()));
    thread::scope(|s| -> Result<(), String> {
        let mut handles = vec![];
        for chunk in ips.chunks(CHUNK) {
            let chunk = chunk.to_vec();
            handles.push(s.spawn(move || {
                chunk
                    .into_iter()
                    .map(|ip| ScanHost {
                        alive: ping_host(&ip),
                        ip,
                    })
                    .collect::<Vec<_>>()
            }));
        }
        for h in handles {
            let part = h.join().map_err(|_| "scan thread panicked".to_string())?;
            results
                .lock()
                .map_err(|_| "mutex poisoned".to_string())?
                .extend(part);
        }
        Ok(())
    })?;

    let mut hosts = results
        .into_inner()
        .map_err(|_| "mutex poisoned".to_string())?;
    hosts.sort_by(|a, b| a.ip.cmp(&b.ip));
    Ok(hosts)
}

/// Derive OpenSSH public key line from a private key PEM (requires `ssh-keygen` on PATH).
pub fn public_key_from_private_pem(pem: &str) -> Result<String, String> {
    let pem = pem.trim();
    if pem.is_empty() {
        return Err("Empty key".into());
    }
    let mut f = NamedTempFile::new().map_err(|e| e.to_string())?;
    f.write_all(pem.as_bytes()).map_err(|e| e.to_string())?;
    f.flush().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600));
    }
    let out = Command::new("ssh-keygen")
        .args(["-y", "-f"])
        .arg(f.path())
        .output()
        .map_err(|e| {
            format!(
                "Could not run ssh-keygen: {}. Install OpenSSH client and ensure ssh-keygen is on PATH.",
                e
            )
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "ssh-keygen failed: {}",
            err.trim().chars().take(200).collect::<String>()
        ));
    }
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if line.is_empty() {
        return Err("ssh-keygen produced no output".into());
    }
    Ok(line)
}

pub fn ssh_public_key_for_credential(
    db: &DbPool,
    cred_id: i64,
    project_id: i64,
) -> Result<String, String> {
    if crud::get_project(db, project_id).is_none() {
        return Err("Project not found".into());
    }
    let cred = crud::get_credential(db, cred_id).ok_or_else(|| "Credential not found".to_string())?;
    if cred.project_id != project_id {
        // Same message as missing id to avoid leaking which credential IDs exist.
        return Err("Credential not found".into());
    }
    if cred.kind != "ssh" {
        return Err("Credential is not an SSH private key".into());
    }
    let secret = crud::get_credential_secret(db, cred_id).ok_or_else(|| "Could not read secret".to_string())?;
    public_key_from_private_pem(&secret)
}

/// New Ed25519 key pair (OpenSSH format). Requires `ssh-keygen` on PATH. Temp files are wiped on drop.
pub fn generate_ed25519_keypair() -> Result<(String, String), String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let key_path = dir.path().join("ansible_ui_key");
    let st = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-f"])
        .arg(&key_path)
        .arg("-q")
        .status()
        .map_err(|e| format!("ssh-keygen: {} (install OpenSSH client)", e))?;
    if !st.success() {
        return Err("ssh-keygen failed".into());
    }
    let private_key = std::fs::read_to_string(&key_path).map_err(|e| e.to_string())?;
    let pub_path = key_path.with_extension("pub");
    let public_key = std::fs::read_to_string(&pub_path).map_err(|e| e.to_string())?;
    Ok((public_key.trim().to_string(), private_key))
}

#[cfg(unix)]
fn ansible_user_from_extra(extra: &str) -> String {
    for line in extra.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("ansible_user:") {
            let u = v
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !u.is_empty() && u.len() < 256 {
                return u;
            }
        }
    }
    "root".to_string()
}

#[cfg(unix)]
fn validate_public_key_line(pk: &str) -> Result<String, String> {
    let line = pk.trim();
    if line.is_empty() {
        return Err("public_key is empty".into());
    }
    if line.len() > 8192 {
        return Err("public_key is too long".into());
    }
    if line.contains('\n') || line.contains('\r') {
        return Err("public_key must be a single line".into());
    }
    if !(line.starts_with("ssh-ed25519 ")
        || line.starts_with("ssh-rsa ")
        || line.starts_with("ecdsa-sha2-nistp256 ")
        || line.starts_with("ecdsa-sha2-nistp384 ")
        || line.starts_with("ecdsa-sha2-nistp521 ")
        || line.starts_with("sk-ssh-ed25519@openssh.com ")
        || line.starts_with("sk-ecdsa-sha2-nistp256@openssh.com ")
        || line.starts_with("sk-ecdsa-sha2-nistp384@openssh.com "))
    {
        return Err(
            "public_key must be one OpenSSH line (ssh-ed25519, ssh-rsa, ecdsa-sha2-…, or sk-* security key)".into(),
        );
    }
    Ok(line.to_string())
}

#[cfg(unix)]
const MAX_DEPLOY_HOSTS: usize = 32;

#[derive(Debug, serde::Serialize)]
pub struct DeployKeyResult {
    pub ip: String,
    pub ok: bool,
    pub detail: String,
}

/// Append one public key line to `~/.ssh/authorized_keys` on the remote host via `ssh`.
/// Skips append when an identical line already exists (`grep -qxF`) so repeat deploys do not duplicate.
/// **SSH key credential:** `ssh -i key …`. **Password credential:** `sshpass -e ssh …` (install `sshpass`).
#[cfg(unix)]
fn append_pubkey_via_ssh(
    use_password: bool,
    password: Option<&str>,
    identity: Option<&std::path::Path>,
    user: &str,
    ip: &str,
    pubkey_line: &str,
) -> Result<(), String> {
    let dest = format!("{user}@{ip}");
    // Use $HOME so authorized_keys is always under the login user’s home (cwd for non-interactive ssh is not guaranteed).
    // LINE=$(cat) reads the one line we send on stdin; grep -qxF avoids duplicate identical lines.
    const REMOTE: &str = "mkdir -p \"$HOME/.ssh\" && chmod 700 \"$HOME/.ssh\" 2>/dev/null; umask 077; touch \"$HOME/.ssh/authorized_keys\" 2>/dev/null; chmod 600 \"$HOME/.ssh/authorized_keys\" 2>/dev/null; LINE=$(cat); if [ -n \"$LINE\" ] && ! grep -qxF -- \"$LINE\" \"$HOME/.ssh/authorized_keys\" 2>/dev/null; then printf '%s\\n' \"$LINE\" >> \"$HOME/.ssh/authorized_keys\"; fi; chmod 600 \"$HOME/.ssh/authorized_keys\"";

    let mut child = if use_password {
        let pw = password.ok_or("missing password")?;
        Command::new("sshpass")
            .env("SSHPASS", pw)
            .args([
                "-e",
                "ssh",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=18",
                &dest,
                REMOTE,
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| {
                format!(
                    "sshpass/ssh: {} — install `sshpass` for password-based deploy",
                    e
                )
            })?
    } else {
        let key_path = identity.ok_or("missing SSH key")?;
        let ks = key_path.to_str().ok_or("invalid key path")?;
        Command::new("ssh")
            .args([
                "-i",
                ks,
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=18",
                &dest,
                REMOTE,
            ])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("ssh: {}", e))?
    };

    let mut stdin = child.stdin.take().ok_or("ssh stdin")?;
    stdin
        .write_all(pubkey_line.as_bytes())
        .map_err(|e| e.to_string())?;
    stdin.write_all(b"\n").map_err(|e| e.to_string())?;
    drop(stdin);

    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().chars().take(400).collect());
    }
    Ok(())
}

/// Install `public_key` on each host using the credential (SSH private key or SSH password).
/// Remote user comes from `ansible_user` in credential **Extra** YAML (default `root`).
/// Identical key lines are not appended again (remote `grep -qxF`).
#[cfg(unix)]
pub fn deploy_public_key_to_hosts(
    db: &DbPool,
    project_id: i64,
    credential_id: i64,
    ips: Vec<String>,
    public_key: &str,
) -> Result<Vec<DeployKeyResult>, String> {
    if crud::get_project(db, project_id).is_none() {
        return Err("Project not found".into());
    }
    let cred = crud::get_credential(db, credential_id).ok_or_else(|| "Credential not found".to_string())?;
    if cred.project_id != project_id {
        return Err("Credential not found".into());
    }
    if cred.kind != "ssh" && cred.kind != "password" {
        return Err("Credential must be SSH private key or SSH password".into());
    }
    let secret = crud::get_credential_secret(db, credential_id).ok_or_else(|| "Could not read secret".to_string())?;
    let user = ansible_user_from_extra(&cred.extra);
    if user.is_empty()
        || !user
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err("Set ansible_user in credential Extra (e.g. ubuntu) — invalid or empty username".into());
    }
    let pk = validate_public_key_line(public_key)?;
    if ips.is_empty() {
        return Err("ips is empty".into());
    }
    if ips.len() > MAX_DEPLOY_HOSTS {
        return Err(format!("Too many hosts (max {MAX_DEPLOY_HOSTS})"));
    }
    for ip in &ips {
        ip.parse::<Ipv4Addr>()
            .map_err(|_| format!("Invalid IPv4 address: {ip}"))?;
    }

    let tmp_key = if cred.kind == "ssh" {
        let mut f = NamedTempFile::new().map_err(|e| e.to_string())?;
        f.write_all(secret.trim().as_bytes())
            .map_err(|e| e.to_string())?;
        f.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(f.path(), std::fs::Permissions::from_mode(0o600));
        }
        Some(f)
    } else {
        None
    };
    let identity = tmp_key.as_ref().map(|f| f.path());

    let mut results = Vec::new();
    for ip in ips {
        let res = if cred.kind == "password" {
            append_pubkey_via_ssh(true, Some(secret.trim()), None, &user, &ip, &pk)
        } else {
            append_pubkey_via_ssh(false, None, identity, &user, &ip, &pk)
        };
        results.push(DeployKeyResult {
            detail: match &res {
                Ok(()) => "ok".into(),
                Err(e) => e.clone(),
            },
            ip,
            ok: res.is_ok(),
        });
    }
    Ok(results)
}

#[cfg(not(unix))]
pub fn deploy_public_key_to_hosts(
    _db: &DbPool,
    _project_id: i64,
    _credential_id: i64,
    _ips: Vec<String>,
    _public_key: &str,
) -> Result<Vec<DeployKeyResult>, String> {
    Err("Deploy is only supported on Linux and macOS".into())
}
