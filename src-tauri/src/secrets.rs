//! Encrypt/decrypt credential secrets (AES-256-GCM).
//!
//! Key resolution (first match):
//! 1. `ANSIBLE_UI_SECRET_KEY` — at least 32 UTF-8 bytes (first 32 bytes used).
//! 2. Key file: `ANSIBLE_UI_KEYFILE`, else `<database-dir>/ansible_ui_secret.key`.
//! 3. Generate 32 random bytes, write key file (base64, one line), use that key.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm,
};
use aead::generic_array::GenericArray;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use rand::RngCore;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ENV_KEY: &str = "ANSIBLE_UI_SECRET_KEY";
const ENV_KEYFILE: &str = "ANSIBLE_UI_KEYFILE";

static MASTER_KEY: OnceLock<[u8; 32]> = OnceLock::new();

/// Parent directory of the SQLite file (aligned with `db` path resolution).
fn database_parent_dir() -> PathBuf {
    let path = env::var("DATABASE_URL")
        .ok()
        .and_then(|u| {
            if let Some(s) = u.strip_prefix("sqlite:///") {
                Some(Path::new(s).to_path_buf())
            } else if let Some(s) = u.strip_prefix("sqlite://") {
                Some(Path::new(s).to_path_buf())
            } else {
                None
            }
        })
        .unwrap_or_else(|| Path::new("./data/ansible_ui.db").to_path_buf());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn default_keyfile_path() -> PathBuf {
    database_parent_dir().join("ansible_ui_secret.key")
}

fn keyfile_path() -> PathBuf {
    env::var(ENV_KEYFILE)
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_keyfile_path())
}

/// Try to interpret `raw` as 32 key bytes: base64 decode, or raw file bytes (first 32).
fn key_from_file_contents(raw: &[u8]) -> Option<[u8; 32]> {
    let s = String::from_utf8_lossy(raw);
    let trimmed = s.trim();
    if let Ok(decoded) = BASE64.decode(trimmed.as_bytes()) {
        if decoded.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&decoded);
            return Some(key);
        }
    }
    if trimmed.len() == 64 {
        fn nibble(b: u8) -> Option<u8> {
            match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            }
        }
        let mut bytes = [0u8; 32];
        let t = trimmed.as_bytes();
        let mut ok = true;
        for i in 0..32 {
            if let (Some(hi), Some(lo)) = (nibble(t[i * 2]), nibble(t[i * 2 + 1])) {
                bytes[i] = (hi << 4) | lo;
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(bytes);
        }
    }
    if raw.len() >= 32 {
        let mut key = [0u8; 32];
        key.copy_from_slice(&raw[..32]);
        return Some(key);
    }
    None
}

fn read_key_from_file(path: &Path) -> Option<[u8; 32]> {
    let raw = std::fs::read(path).ok()?;
    key_from_file_contents(&raw)
}

fn write_generated_key_file(path: &Path, key: &[u8; 32]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = format!("{}\n", BASE64.encode(key));
    let tmp = path.with_extension("key.tmp");
    std::fs::write(&tmp, &line)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    // Windows: rename over existing is OK
    let _ = std::fs::remove_file(path);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn load_or_create_master_key() -> [u8; 32] {
    if let Ok(raw) = env::var(ENV_KEY) {
        let bytes = raw.as_bytes();
        if bytes.len() >= 32 {
            let mut key = [0u8; 32];
            key[..32].copy_from_slice(&bytes[..32]);
            tracing::info!(
                "{} loaded from environment (32-byte prefix used).",
                ENV_KEY
            );
            return key;
        }
        tracing::warn!(
            "{} is set but shorter than 32 bytes; ignoring for key selection.",
            ENV_KEY
        );
    }

    let path = keyfile_path();
    if path.is_file() {
        if let Some(key) = read_key_from_file(&path) {
            tracing::info!("Encryption key loaded from {}.", path.display());
            return key;
        }
        tracing::warn!(
            "Could not parse encryption key from {}; will generate a new one.",
            path.display()
        );
    }

    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    if let Err(e) = write_generated_key_file(&path, &key) {
        tracing::error!(
            "Failed to persist encryption key to {}: {}. Using in-memory key only (credentials may not decrypt after restart).",
            path.display(),
            e
        );
    } else {
        tracing::warn!(
            "Generated new AES key and wrote {}. Back up this file; without it stored credentials cannot be decrypted. For containers, mount a volume or set {}.",
            path.display(),
            ENV_KEY
        );
    }
    key
}

fn master_key() -> &'static [u8; 32] {
    MASTER_KEY.get_or_init(load_or_create_master_key)
}

pub fn encrypt_secret(plain: &str) -> String {
    let key = master_key();
    let cipher = Aes256Gcm::new_from_slice(key.as_slice()).expect("key length");
    let nonce: [u8; 12] = rand::random();
    let ciphertext = cipher
        .encrypt((&nonce).into(), plain.as_bytes())
        .expect("encrypt");
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    BASE64.encode(&combined)
}

pub fn decrypt_secret(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() {
        return Ok(String::new());
    }
    let combined = BASE64.decode(encoded).map_err(|e| e.to_string())?;
    if combined.len() < 12 + 16 {
        return Err("invalid ciphertext".into());
    }
    let key = master_key();
    let cipher = Aes256Gcm::new_from_slice(key.as_slice()).expect("key length");
    let (nonce, ct) = combined.split_at(12);
    let nonce_arr = GenericArray::from_slice(nonce);
    let plain = cipher
        .decrypt(nonce_arr, ct)
        .map_err(|_| "decrypt failed")?;
    String::from_utf8(plain).map_err(|e| e.to_string())
}
