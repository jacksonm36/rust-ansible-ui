//! Encrypt/decrypt credential secrets (AES-256-GCM). Key from env or derived.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm,
};
use aead::generic_array::GenericArray;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::env;

const ENV_KEY: &str = "ANSIBLE_UI_SECRET_KEY";
const DEFAULT_SALT: &[u8] = b"ansible-ui-credential-salt";

fn get_key() -> [u8; 32] {
    if let Ok(raw) = env::var(ENV_KEY) {
        let bytes = raw.as_bytes();
        if bytes.len() >= 32 {
            let mut key = [0u8; 32];
            key[..32].copy_from_slice(&bytes[..32]);
            return key;
        }
    }
    tracing::warn!(
        "SECURITY WARNING: {} is not set or is shorter than 32 characters. \
         A predictable default encryption key is being used. Set it in production.",
        ENV_KEY
    );
    // PBKDF2-like: use a fixed derivation (not cryptographically strong for production)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    DEFAULT_SALT.hash(&mut hasher);
    b"ansible-ui-default-key".hash(&mut hasher);
    let h = hasher.finish();
    let mut key = [0u8; 32];
    for i in 0..32 {
        key[i] = (h >> (i * 8)) as u8;
    }
    key
}

pub fn encrypt_secret(plain: &str) -> String {
    let key = get_key();
    let cipher = Aes256Gcm::new_from_slice(&key).expect("key length");
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
    let key = get_key();
    let cipher = Aes256Gcm::new_from_slice(&key).expect("key length");
    let (nonce, ct) = combined.split_at(12);
    let nonce_arr = GenericArray::from_slice(nonce);
    let plain = cipher
        .decrypt(nonce_arr, ct)
        .map_err(|_| "decrypt failed")?;
    String::from_utf8(plain).map_err(|e| e.to_string())
}
