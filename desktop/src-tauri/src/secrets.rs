//! Field-level encryption for the JSON vault.
//!
//! The document on disk (and in Drive) stays readable: hosts, ports, settings
//! and tags are plain JSON. Only credential fields are replaced by
//! `enc:v1:<nonce_b64>:<ciphertext_b64>` values sealed with the vault key.
//! Every ciphertext is bound to a stable key epoch, the entity id and the
//! field name via AEAD associated data, so swapping values between fields or
//! hosts fails authentication (F-29/F-40).

use rterm_core::crypto::{
    open_bytes, seal_bytes, CryptoError, KdfParams, SecretKey, SealedVault,
};
use rterm_core::model::{AuthMethod, Vault};

pub const PREFIX: &str = "enc:v1:";

/// Stable identifier for the key generation. Derived from the KDF salt, so it
/// never changes on ordinary syncs but does change if the passphrase/key is
/// re-derived. Recovery-key vaults use a fixed marker.
pub fn kdf_epoch(kdf: Option<&KdfParams>) -> String {
    kdf.map(|p| p.salt_b64.as_str())
        .unwrap_or("recovery-key")
        .to_string()
}

fn field_aad(epoch: &str, entity: &str, field: &str) -> Vec<u8> {
    format!("rterm-field-v1\x1f{epoch}\x1f{entity}\x1f{field}").into_bytes()
}

pub fn canary_aad() -> Vec<u8> {
    b"rterm-canary-v1\x1fglobal".to_vec()
}

/// Seals the known canary string with the vault key.
pub fn seal_canary(key: &SecretKey, value: &str) -> Result<String, CryptoError> {
    seal_str(key, value, &canary_aad())
}

/// Constant-time comparison for the canary; a plain `==` leaks prefix length
/// through timing. Length equality is already public.
fn constant_time_eq(expected: &str, actual: &str) -> bool {
    let a = expected.as_bytes();
    let b = actual.as_bytes();
    let mut diff = std::hint::black_box(a.len() ^ b.len());
    for i in 0..a.len().max(b.len()) {
        let x = std::hint::black_box(a.get(i).copied().unwrap_or(0));
        let y = std::hint::black_box(b.get(i).copied().unwrap_or(0));
        diff |= std::hint::black_box((x ^ y) as usize);
    }
    std::hint::black_box(diff) == 0
}

/// Seals a non-empty secret. An already sealed value is kept only if it
/// authenticates under the *same* context; otherwise the write is rejected.
pub fn seal_str(
    key: &SecretKey,
    value: &str,
    aad: &[u8],
) -> Result<String, CryptoError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.starts_with(PREFIX) {
        open_str(key, value, aad)?; // idempotent, but context-bound
        return Ok(value.to_string());
    }
    let sealed = seal_bytes(value.as_bytes(), key, None, aad)?;
    Ok(format!(
        "{PREFIX}{}:{}",
        sealed.nonce_b64, sealed.ciphertext_b64
    ))
}

/// Opens a sealed secret. Plaintext values are rejected (except the empty
/// string, which means "no secret configured").
pub fn open_str(
    key: &SecretKey,
    value: &str,
    aad: &[u8],
) -> Result<String, CryptoError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let rest = value.strip_prefix(PREFIX).ok_or(CryptoError::Decrypt)?;
    let (nonce, ciphertext) = rest.split_once(':').ok_or(CryptoError::Decrypt)?;
    if ciphertext.contains(':') {
        return Err(CryptoError::Decrypt);
    }
    let sealed = SealedVault {
        v: 1,
        kdf: None,
        nonce_b64: nonce.to_string(),
        ciphertext_b64: ciphertext.to_string(),
    };
    let bytes = open_bytes(&sealed, key, aad)?;
    String::from_utf8(bytes).map_err(|_| CryptoError::Decrypt)
}

/// Strict canary verification: rejects plaintext, wrong key and wrong context,
/// and compares the plaintext in constant time.
pub fn verify_canary(key: &SecretKey, sealed: &str, expected: &str) -> bool {
    match open_str(key, sealed, &canary_aad()) {
        Ok(value) => constant_time_eq(expected, &value),
        Err(_) => false,
    }
}

/// Replaces every credential in a plaintext vault with its sealed form.
pub fn seal_vault(vault: &Vault, key: &SecretKey, epoch: &str) -> Result<Vault, CryptoError> {
    let mut out = vault.clone();
    for host in &mut out.hosts {
        let entity = host.id.to_string();
        host.auth = match &host.auth {
            AuthMethod::Password { password } => AuthMethod::Password {
                password: seal_str(key, password, &field_aad(epoch, &entity, "password"))?,
            },
            AuthMethod::KeyboardInteractive { password } => AuthMethod::KeyboardInteractive {
                password: seal_str(key, password, &field_aad(epoch, &entity, "password"))?,
            },
            AuthMethod::Key { identity_id, passphrase } => AuthMethod::Key {
                identity_id: *identity_id,
                passphrase: match passphrase {
                    Some(value) => Some(seal_str(
                        key,
                        value,
                        &field_aad(epoch, &entity, "passphrase"),
                    )?),
                    None => None,
                },
            },
            other => other.clone(),
        };
    }
    for identity in &mut out.identities {
        let entity = identity.id.to_string();
        identity.private_key = seal_str(
            key,
            &identity.private_key,
            &field_aad(epoch, &entity, "private_key"),
        )?;
        if let Some(passphrase) = &identity.passphrase {
            identity.passphrase = Some(seal_str(
                key,
                passphrase,
                &field_aad(epoch, &entity, "passphrase"),
            )?);
        }
    }
    Ok(out)
}

/// Inverse of [`seal_vault`]: plaintext secrets back in memory.
pub fn open_vault(vault: &Vault, key: &SecretKey, epoch: &str) -> Result<Vault, CryptoError> {
    let mut out = vault.clone();
    for host in &mut out.hosts {
        let entity = host.id.to_string();
        host.auth = match &host.auth {
            AuthMethod::Password { password } => AuthMethod::Password {
                password: open_str(key, password, &field_aad(epoch, &entity, "password"))?,
            },
            AuthMethod::KeyboardInteractive { password } => AuthMethod::KeyboardInteractive {
                password: open_str(key, password, &field_aad(epoch, &entity, "password"))?,
            },
            AuthMethod::Key { identity_id, passphrase } => AuthMethod::Key {
                identity_id: *identity_id,
                passphrase: match passphrase {
                    Some(value) => Some(open_str(
                        key,
                        value,
                        &field_aad(epoch, &entity, "passphrase"),
                    )?),
                    None => None,
                },
            },
            other => other.clone(),
        };
    }
    for identity in &mut out.identities {
        let entity = identity.id.to_string();
        identity.private_key = open_str(
            key,
            &identity.private_key,
            &field_aad(epoch, &entity, "private_key"),
        )?;
        if let Some(passphrase) = &identity.passphrase {
            identity.passphrase = Some(open_str(
                key,
                passphrase,
                &field_aad(epoch, &entity, "passphrase"),
            )?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rterm_core::model::{AuthMethod, Host, Identity};
    use uuid::Uuid;

    fn sample() -> Vault {
        let mut vault = Vault::default();
        let id = Uuid::new_v4();
        vault.hosts.push(Host {
            id,
            label: "web".into(),
            address: "10.0.0.1".into(),
            port: 22,
            username: "root".into(),
            auth: AuthMethod::Password { password: "s3cret".into() },
            group_id: None,
            tags: vec![],
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
            created_at: rterm_core::model::now(),
            updated_at: rterm_core::model::now(),
            last_used_at: None,
            deleted: false,
        });
        vault.identities.push(Identity {
            id: Uuid::new_v4(),
            label: "key".into(),
            private_key: "PRIVATE".into(),
            passphrase: Some("pass".into()),
            kind: None,
            fingerprint: None,
            created_at: rterm_core::model::now(),
            updated_at: rterm_core::model::now(),
            deleted: false,
        });
        vault
    }

    #[test]
    fn secret_fields_roundtrip_and_are_bound_to_context() {
        let key = SecretKey::generate();
        let sealed = seal_vault(&sample(), &key, "epoch-1").unwrap();
        let json = serde_json::to_string(&sealed).unwrap();
        assert!(!json.contains("s3cret"));
        assert!(!json.contains("PRIVATE"));
        let opened = open_vault(&sealed, &key, "epoch-1").unwrap();
        assert!(matches!(
            &opened.hosts[0].auth,
            AuthMethod::Password { password } if password == "s3cret"
        ));
        assert!(open_vault(&sealed, &key, "epoch-2").is_err());
    }

    #[test]
    fn plaintext_secret_is_rejected_on_read() {
        let key = SecretKey::generate();
        let mut sealed = seal_vault(&sample(), &key, "epoch-1").unwrap();
        sealed.hosts[0].auth = AuthMethod::Password { password: "plain".into() };
        assert!(open_vault(&sealed, &key, "epoch-1").is_err());
    }
}
