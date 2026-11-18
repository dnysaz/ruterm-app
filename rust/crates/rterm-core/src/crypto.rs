//! Client-side end-to-end encryption for the vault.
//!
//! The server only ever stores [`SealedVault`]: a JSON envelope containing an
//! XChaCha20-Poly1305 ciphertext plus the Argon2id parameters used to turn the
//! user's passphrase into the vault key. The passphrase never leaves the
//! device, so a compromised server cannot read hostnames, passwords or keys.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::model::Vault;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const SALT_LEN: usize = 16;
pub const SCHEMA: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("key derivation failed: {0}")]
    Kdf(String),
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed (wrong passphrase/recovery key, or corrupted vault)")]
    Decrypt,
    #[error("invalid key length")]
    KeyLength,
    #[error("serialisation failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported vault schema {0}")]
    Schema(u32),
}

/// 32-byte symmetric key. Zeroized on drop and never printed.
#[derive(Clone)]
pub struct SecretKey([u8; KEY_LEN]);

impl SecretKey {
    pub fn generate() -> Self {
        let mut key = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut key);
        Self(key)
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    pub fn from_base64(s: &str) -> Result<Self, CryptoError> {
        let bytes = B64.decode(s.trim())?;
        let arr: [u8; KEY_LEN] = bytes.try_into().map_err(|_| CryptoError::KeyLength)?;
        Ok(Self(arr))
    }

    /// Human friendly recovery key: 32 random bytes as Crockford base32,
    /// grouped in blocks of 5. Case-insensitive and has no easily confused
    /// characters (no I, L, O, U).
    pub fn generate_recovery_key() -> String {
        let mut raw = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut raw);
        let encoded = base32_encode(&raw);
        raw.zeroize();
        encoded
            .as_bytes()
            .chunks(5)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// Normalises a recovery key typed by a human (strips dashes/spaces,
    /// upper-cases, maps O->0 and I/L->1).
    pub fn from_recovery_key(s: &str) -> Result<Self, CryptoError> {
        let cleaned: String = s
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect();
        let bytes = base32_decode(&cleaned).ok_or(CryptoError::KeyLength)?;
        let arr: [u8; KEY_LEN] = bytes.try_into().map_err(|_| CryptoError::KeyLength)?;
        Ok(Self(arr))
    }
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(redacted)")
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Argon2id parameters, stored alongside the ciphertext.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub alg: String,
    pub salt_b64: String,
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// OWASP 2024 baseline: 19 MiB, 2 iterations.
    pub fn new_default() -> Self {
        let mut salt = [0u8; SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);
        Self {
            alg: "argon2id".into(),
            salt_b64: B64.encode(salt),
            m_cost: 19 * 1024,
            t_cost: 2,
            p_cost: 1,
        }
    }

    pub fn salt(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(B64.decode(&self.salt_b64)?)
    }

    /// Cheap parameters, for unit tests only.
    #[cfg(test)]
    pub fn new_test() -> Self {
        let mut params = Self::new_default();
        params.m_cost = 8;
        params.t_cost = 1;
        params
    }
}

/// What the sync server stores. Contains no plaintext and no key material.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SealedVault {
    pub v: u32,
    /// `None` means the key is a high-entropy recovery key used directly.
    pub kdf: Option<KdfParams>,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
}

pub fn derive_key(passphrase: &str, params: &KdfParams) -> Result<SecretKey, CryptoError> {
    if params.alg != "argon2id" {
        return Err(CryptoError::Kdf(format!("unsupported kdf {}", params.alg)));
    }
    let salt = params.salt()?;
    let argon_params = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(KEY_LEN))
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(passphrase.as_bytes(), &salt, &mut out)
        .map_err(|e| CryptoError::Kdf(e.to_string()))?;
    Ok(SecretKey(out))
}

pub fn seal_bytes(
    plaintext: &[u8],
    key: &SecretKey,
    kdf: Option<KdfParams>,
) -> Result<SealedVault, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    Ok(SealedVault {
        v: SCHEMA,
        kdf,
        nonce_b64: B64.encode(nonce_bytes),
        ciphertext_b64: B64.encode(ciphertext),
    })
}

pub fn open_bytes(sealed: &SealedVault, key: &SecretKey) -> Result<Vec<u8>, CryptoError> {
    if sealed.v > SCHEMA {
        return Err(CryptoError::Schema(sealed.v));
    }
    let nonce_bytes = B64.decode(&sealed.nonce_b64)?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(CryptoError::Decrypt);
    }
    let ciphertext = B64.decode(&sealed.ciphertext_b64)?;
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    cipher
        .decrypt(XNonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| CryptoError::Decrypt)
}

pub fn seal_vault(vault: &Vault, key: &SecretKey, kdf: Option<KdfParams>) -> Result<SealedVault, CryptoError> {
    let plaintext = serde_json::to_vec(vault)?;
    seal_bytes(&plaintext, key, kdf)
}

pub fn open_vault(sealed: &SealedVault, key: &SecretKey) -> Result<Vault, CryptoError> {
    let plaintext = open_bytes(sealed, key)?;
    Ok(serde_json::from_slice(&plaintext)?)
}

/// Convenience: derive a key from a passphrase and seal in one call.
pub fn seal_vault_with_passphrase(
    vault: &Vault,
    passphrase: &str,
) -> Result<(SealedVault, SecretKey), CryptoError> {
    let params = KdfParams::new_default();
    let key = derive_key(passphrase, &params)?;
    let sealed = seal_vault(vault, &key, Some(params))?;
    Ok((sealed, key))
}

/// Unlock a sealed vault with a passphrase, returning both the vault and the
/// cached key (so the session can keep syncing without re-deriving).
pub fn open_vault_with_passphrase(
    sealed: &SealedVault,
    passphrase: &str,
) -> Result<(Vault, SecretKey), CryptoError> {
    let key = match &sealed.kdf {
        Some(params) => derive_key(passphrase, params)?,
        None => SecretKey::from_recovery_key(passphrase)?,
    };
    let vault = open_vault(sealed, &key)?;
    Ok((vault, key))
}


const BASE32_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for byte in bytes {
        buffer = (buffer << 8) | *byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32_ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for ch in input.chars() {
        let upper = ch.to_ascii_uppercase();
        let value = match upper {
            '0' | 'O' => 0,
            '1' | 'I' | 'L' => 1,
            'U' => return None,
            'A'..='Z' | '2'..='9' => {
                BASE32_ALPHABET.iter().position(|c| *c as char == upper)? as u32
            }
            _ => return None,
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod base32_tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let bytes: Vec<u8> = (0u8..32).collect();
        let encoded = base32_encode(&bytes);
        assert_eq!(base32_decode(&encoded).unwrap(), bytes);
    }

    #[test]
    fn recovery_key_is_case_insensitive() {
        let rk = SecretKey::generate_recovery_key();
        let a = SecretKey::from_recovery_key(&rk).unwrap();
        let b = SecretKey::from_recovery_key(&rk.to_lowercase()).unwrap();
        assert_eq!(a.to_base64(), b.to_base64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, Host};

    fn sample_vault() -> Vault {
        let mut vault = Vault::default();
        vault.hosts.push(Host {
            id: uuid::Uuid::new_v4(),
            label: "web-1".into(),
            address: "10.0.0.1".into(),
            port: 22,
            username: "root".into(),
            auth: AuthMethod::Password {
                password: "s3cret".into(),
            },
            group_id: None,
            tags: vec!["prod".into()],
            color: None,
            icon: None,
            favorite: false,
            notes: String::new(),
            startup_snippet_id: None,
            jump_host_id: None,
            keepalive_secs: None,
            connect_timeout_secs: None,
            env: vec![],
            terminal: Default::default(),
            agent_forwarding: false,
            compression: false,
            os: None,
            host_key_fingerprint: None,
            created_at: crate::model::now(),
            updated_at: crate::model::now(),
            last_used_at: None,
            deleted: false,
        });
        vault
    }

    #[test]
    fn roundtrip_with_passphrase() {
        let vault = sample_vault();
        let params = KdfParams::new_test();
        let key = derive_key("correct horse battery staple", &params).unwrap();
        let sealed = seal_vault(&vault, &key, Some(params)).unwrap();
        let (opened, _) = open_vault_with_passphrase(&sealed, "correct horse battery staple").unwrap();
        assert_eq!(opened.hosts[0].address, "10.0.0.1");
    }

    #[test]
    fn wrong_passphrase_fails() {
        let vault = sample_vault();
        let (sealed, _) = seal_vault_with_passphrase(&vault, "right").unwrap();
        // Uses default (expensive) params, so just check it errors.
        assert!(open_vault_with_passphrase(&sealed, "wrong").is_err());
    }

    #[test]
    fn ciphertext_hides_secrets() {
        let vault = sample_vault();
        let key = SecretKey::generate();
        let sealed = seal_vault(&vault, &key, None).unwrap();
        let json = serde_json::to_string(&sealed).unwrap();
        assert!(!json.contains("s3cret"));
        assert!(!json.contains("10.0.0.1"));
    }

}
