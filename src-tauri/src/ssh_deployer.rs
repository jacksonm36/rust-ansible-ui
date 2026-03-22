//! LAN ping sweep (ICMP via system `ping`) and SSH public key extraction (`ssh-keygen -y`).

use crate::crud;
use crate::db::DbPool;
use std::io::Write;
use std::net::Ipv4Addr;
use std::process::Command;
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
    #[cfg(not(windows))]
    {
        // GNU ping: -W timeout in seconds
        if Command::new("ping")
            .args(["-c", "1", "-W", "1", ip])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return true;
        }
        // BSD/macOS: -W is packet wait; try -t (wait seconds) where supported
        Command::new("ping")
            .args(["-c", "1", "-t", "1", ip])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
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
