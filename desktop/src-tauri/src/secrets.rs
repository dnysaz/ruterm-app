//! Field-level encryption for the JSON vault.
//!
//! The document on disk (and in Drive) stays readable: hosts, ports, settings
//! and tags are plain JSON. Only the sensitive strings are replaced by
//! `enc:v1:<nonce_b64>:<ciphertext_b64>` values sealed with the vault key
//! (XChaCha20-Poly1305, same primitive as `rterm-core::crypto`).

use rterm_core::crypto::{open_bytes, seal_bytes, CryptoError, SecretKey, SealedVault};
use rterm_core::model::{AuthMethod, Vault};

const PREFIX: &str = "enc:v1:";

pub fn seal_str(key: &SecretKey, value: &str) -> Result<String, CryptoError> {
    if value.is_empty() || value.starts_with(PREFIX) {
        return Ok(value.to_string());
    }
    let sealed = seal_bytes(value.as_bytes(), key, None)?;
    Ok(format!(
        "{PREFIX}{}:{}",
        sealed.nonce_b64, sealed.ciphertext_b64
    ))
}

pub fn open_str(key: &SecretKey, value: &str) -> Result<String, CryptoError> {
    let Some(rest) = value.strip_prefix(PREFIX) else {
        return Ok(value.to_string());
    };
    let (nonce, ciphertext) = rest.split_once(':').ok_or(CryptoError::Decrypt)?;
    let sealed = SealedVault {
        v: 1,
        kdf: None,
        nonce_b64: nonce.to_string(),
        ciphertext_b64: ciphertext.to_string(),
    };
    let bytes = open_bytes(&sealed, key)?;
    String::from_utf8(bytes).map_err(|_| CryptoError::Decrypt)
}

/// Replaces every secret in a plaintext vault with its sealed form.
pub fn seal_vault(vault: &Vault, key: &SecretKey) -> Result<Vault, CryptoError> {
    let mut out = vault.clone();
    for host in &mut out.hosts {
        host.auth = match &host.auth {
            AuthMethod::Password { password } => AuthMethod::Password {
                password: seal_str(key, password)?,
            },
            AuthMethod::KeyboardInteractive { password } => AuthMethod::KeyboardInteractive {
                password: seal_str(key, password)?,
            },
            other => other.clone(),
        };
    }
    for identity in &mut out.identities {
        identity.private_key = seal_str(key, &identity.private_key)?;
        if let Some(passphrase) = &identity.passphrase {
            identity.passphrase = Some(seal_str(key, passphrase)?);
        }
    }
    Ok(out)
}

/// Inverse of [`seal_vault`]: plaintext secrets back in memory.
pub fn open_vault(vault: &Vault, key: &SecretKey) -> Result<Vault, CryptoError> {
    let mut out = vault.clone();
    for host in &mut out.hosts {
        host.auth = match &host.auth {
            AuthMethod::Password { password } => AuthMethod::Password {
                password: open_str(key, password)?,
            },
            AuthMethod::KeyboardInteractive { password } => AuthMethod::KeyboardInteractive {
                password: open_str(key, password)?,
            },
            other => other.clone(),
        };
    }
    for identity in &mut out.identities {
        identity.private_key = open_str(key, &identity.private_key)?;
        if let Some(passphrase) = &identity.passphrase {
            identity.passphrase = Some(open_str(key, passphrase)?);
        }
    }
    Ok(out)
}
