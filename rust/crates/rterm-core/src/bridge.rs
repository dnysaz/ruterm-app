//! Sync client for the RuTerm bridge: a Cloudflare Worker backed by D1.
//!
//! The bridge is a credential-encrypted blob store (NOT zero-knowledge):
//! passwords, private keys and secret note fields are sealed on this device
//! before upload, but host metadata (addresses, usernames, tags) is plain JSON
//! and readable by the operator. The server stores those sealed values plus a
//! revision counter.
//!
//! Protocol:
//!
//! * `POST /v1/auth/google` — exchange a loopback+PKCE authorization code
//!   (RFC 8252) for a RuTerm session. The client holds no secret.
//! * `POST /v1/auth/refresh` — rotate the refresh token, new access token.
//! * `POST /v1/auth/logout` — revoke the presented refresh token.
//! * `GET  /v1/vault` — `{ revision, blob, updated_at }`
//! * `PUT  /v1/vault` — `{ base_revision, blob }`; `409` with the current
//!   document when `base_revision` is stale.
//!
//! `blob` is an opaque string to the server; here it is a [`JsonSyncDoc`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::crypto::{mac_equal, sync_mac, KdfParams, SecretKey};
use crate::model::Vault;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("bridge returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("revision conflict")]
    Conflict,
    #[error("not authenticated")]
    Unauthenticated,
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sync did not converge after {0} attempts")]
    TooManyRetries(u32),
    #[error("vault key differs from the synced vault — re-enter the passphrase on this device")]
    KeyMismatch,
    #[error("unsafe KDF parameters in the synced vault")]
    UnsafeKdf,
    #[error("synced vault has no integrity MAC (legacy unsigned document) — unsigned documents are not trusted; create a new vault or restore a signed copy")]
    MissingMac,
    #[error("synced vault integrity MAC is invalid — the document was modified or signed with another key")]
    InvalidMac,
    #[error("invalid synced vault: {0}")]
    InvalidSyncDoc(String),
}

/// Chooses the key state for a merge. A device with a different KDF must not
/// overwrite the remote KDF/canary: that produced permanent desync/lockout
/// (F-38). Fail closed and ask the user to re-enter the passphrase instead.
fn resolve_key_state(
    local_kdf: Option<&KdfParams>,
    local_canary: &str,
    remote_kdf: Option<&KdfParams>,
    remote_canary: &str,
) -> Result<(Option<KdfParams>, String), BridgeError> {
    if let Some(remote) = remote_kdf {
        remote.validate().map_err(|_| BridgeError::UnsafeKdf)?;
    }
    match (local_kdf, remote_kdf) {
        (Some(a), Some(b)) if a != b => return Err(BridgeError::KeyMismatch),
        (None, Some(b)) => Ok((Some(b.clone()), remote_canary.to_string())),
        (Some(a), None) => Ok((Some(a.clone()), local_canary.to_string())),
        (None, None) => Ok((
            None,
            if local_canary.is_empty() {
                remote_canary
            } else {
                local_canary
            }
            .to_string(),
        )),
        (Some(a), Some(_)) => Ok((Some(a.clone()), local_canary.to_string())),
    }
}

/// The document stored as the vault blob. `vault` carries field-level
/// ciphertext (`enc:v1:…`) for secrets, so this is safe to store on the
/// server; only the KDF parameters and an (unreadable) canary ride along so
/// another device can re-derive the same key from the same passphrase.
#[derive(Serialize, Deserialize, Clone)]
pub struct JsonSyncDoc {
    pub schema: u32,
    /// Revision this document was written against. Included in the MAC so a
    /// server (or a tampered client) cannot rewrite the merge clock.
    #[serde(default)]
    pub revision: i64,
    pub device_id: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub kdf: Option<KdfParams>,
    #[serde(default)]
    pub canary: String,
    pub vault: Vault,
    /// `hmac-sha256:<hex>` over the canonical document below. Field-level
    /// ciphertext already authenticates secrets; this authenticates the
    /// *envelope* (revision, timestamps, tombstones and settings) as well.
    #[serde(default)]
    pub mac: String,
}

/// Canonical MAC input. `vault` serialises the complete dataset, including
/// `settings`, every `deleted` tombstone flag and all entity timestamps, so
/// changing any of those invalidates the MAC. `mac` itself is excluded.
#[derive(Serialize)]
struct SyncDocBody<'a> {
    schema: u32,
    revision: i64,
    device_id: &'a str,
    updated_at: &'a DateTime<Utc>,
    kdf: &'a Option<KdfParams>,
    canary: &'a str,
    vault: &'a Vault,
}

const MAC_PREFIX: &str = "hmac-sha256:";
/// Refuse oversized blobs before deserialising them into memory (H2).
const MAX_SYNC_BLOB_BYTES: usize = 8 * 1024 * 1024;

impl JsonSyncDoc {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&SyncDocBody {
            schema: self.schema,
            revision: self.revision,
            device_id: &self.device_id,
            updated_at: &self.updated_at,
            kdf: &self.kdf,
            canary: &self.canary,
            vault: &self.vault,
        })
    }

    /// Signs this document with the vault key. Every upload goes through here.
    pub fn sign(&mut self, key: &SecretKey) -> Result<(), BridgeError> {
        let mac = sync_mac(key, &self.canonical_bytes()?);
        self.mac = format!("{MAC_PREFIX}{}", hex::encode(mac));
        Ok(())
    }

    /// Verifies the whole envelope before any merge/adoption. Missing MACs
    /// (legacy documents) fail closed rather than being treated as trusted.
    pub fn verify_integrity(&self, key: &SecretKey) -> Result<(), BridgeError> {
        let raw = self.mac.strip_prefix(MAC_PREFIX).unwrap_or("");
        if raw.is_empty() {
            return Err(BridgeError::MissingMac);
        }
        let supplied = hex::decode(raw).map_err(|_| BridgeError::InvalidMac)?;
        if supplied.len() != 32 {
            return Err(BridgeError::InvalidMac);
        }
        let expected = sync_mac(key, &self.canonical_bytes()?);
        if !mac_equal(&expected, &supplied) {
            return Err(BridgeError::InvalidMac);
        }
        Ok(())
    }

    /// Structural checks on a remote document, run before it influences a
    /// merge. The KDF is validated here so Argon2 never sees attacker costs.
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.schema != 1 {
            return Err(BridgeError::InvalidSyncDoc(format!(
                "unsupported sync schema {}",
                self.schema
            )));
        }
        if self.revision < 0 || self.device_id.is_empty() || self.device_id.len() > 128 {
            return Err(BridgeError::InvalidSyncDoc("invalid sync envelope".into()));
        }
        if let Some(kdf) = &self.kdf {
            kdf.validate().map_err(|_| BridgeError::UnsafeKdf)?;
        }
        self.vault
            .validate_for_sync()
            .map_err(BridgeError::InvalidSyncDoc)
    }

    /// Remote settings cannot enable local-only chat permissions.
    pub fn remote_vault(&self) -> Vault {
        self.vault.without_remote_permissions()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BridgeUser {
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// Google profile photo URL, when the bridge provides it.
    #[serde(default)]
    pub picture: Option<String>,
    /// The user's Google account id (`sub`), when the bridge provides it.
    #[serde(default)]
    pub sub: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BridgeSession {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub device_id: String,
    pub user: BridgeUser,
}

// F-12: never let a stray `{:?}`/`dbg!` spill bearer or refresh tokens.
impl std::fmt::Debug for BridgeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeSession")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .field("device_id", &self.device_id)
            .field("user", &self.user)
            .finish()
    }
}

impl std::fmt::Debug for JsonSyncDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonSyncDoc")
            .field("schema", &self.schema)
            .field("revision", &self.revision)
            .field("device_id", &self.device_id)
            .field("updated_at", &self.updated_at)
            .field("kdf", &self.kdf)
            .field("canary", &"<redacted>")
            .field("vault", &"<sealed>")
            .field("mac", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: String,
    #[serde(default)]
    pub app_version: String,
}

impl DeviceInfo {
    pub fn current() -> Self {
        Self {
            name: std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("COMPUTERNAME"))
                .unwrap_or_else(|_| "device".into()),
            platform: std::env::consts::OS.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[derive(Serialize)]
struct AuthGoogleBody<'a> {
    code: &'a str,
    code_verifier: &'a str,
    redirect_uri: &'a str,
    device: &'a DeviceInfo,
}

#[derive(Serialize)]
struct RefreshBody<'a> {
    refresh_token: &'a str,
}

#[derive(Deserialize)]
struct VaultResponse {
    #[serde(default)]
    revision: i64,
    #[serde(default)]
    blob: String,
}

#[derive(Serialize)]
struct PutBody<'a> {
    base_revision: i64,
    blob: &'a str,
}

#[derive(Deserialize)]
struct PutResponse {
    revision: i64,
}

pub struct BridgeClient {
    base_url: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl BridgeClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            // F-47: never fall back to `Client::default()` — it silently drops
            // the timeout/user-agent, so a builder failure becomes a hung app.
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent(concat!("rterm/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("failed to construct HTTP client"),
            token: None,
        }
    }

    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, BridgeError> {
        let token = self.token.as_ref().ok_or(BridgeError::Unauthenticated)?;
        Ok(req.bearer_auth(token))
    }

    /// Exchanges the loopback authorization code for a RuTerm session.
    /// Google identity is verified server-side by the bridge; no client
    /// secret exists anywhere in this app.
    pub async fn auth_google(
        &self,
        code: &str,
        code_verifier: &str,
        redirect_uri: &str,
        device: DeviceInfo,
    ) -> Result<BridgeSession, BridgeError> {
        let res = self
            .http
            .post(format!("{}/v1/auth/google", self.base_url))
            .json(&AuthGoogleBody {
                code,
                code_verifier,
                redirect_uri,
                device: &device,
            })
            .send()
            .await?;
        parse(res).await
    }

    /// Rotates the refresh token; the previous one is revoked.
    pub async fn refresh(&self, refresh_token: &str) -> Result<BridgeSession, BridgeError> {
        let res = self
            .http
            .post(format!("{}/v1/auth/refresh", self.base_url))
            .json(&RefreshBody { refresh_token })
            .send()
            .await?;
        parse(res).await
    }

    /// Best-effort revocation of the presented refresh token. The access token
    /// accompanies the request so the server can also revoke the device.
    pub async fn logout(&self, refresh_token: &str) -> Result<(), BridgeError> {
        let res = self
            .auth(self.http.post(format!("{}/v1/auth/logout", self.base_url)))?
            .json(&RefreshBody { refresh_token })
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(status_error(res).await);
        }
        Ok(())
    }

    /// Raw remote document. Callers must run [`JsonSyncDoc::verify_integrity`]
    /// and [`JsonSyncDoc::validate`] before using any field. Adoption needs the
    /// raw copy because the KDF (and therefore the key) rides inside it.
    pub async fn fetch_json_unverified(&self) -> Result<Option<(JsonSyncDoc, i64)>, BridgeError> {
        let res = self
            .auth(self.http.get(format!("{}/v1/vault", self.base_url)))?
            .send()
            .await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let remote: VaultResponse = parse(res).await?;
        if remote.revision == 0 || remote.blob.trim().is_empty() {
            return Ok(None);
        }
        if remote.blob.len() > MAX_SYNC_BLOB_BYTES {
            return Err(BridgeError::InvalidSyncDoc("document is too large".into()));
        }
        let doc: JsonSyncDoc = serde_json::from_str(&remote.blob)?;
        Ok(Some((doc, remote.revision)))
    }

    /// The remote document, if this account has one. The whole envelope is
    /// authenticated and structurally checked before the caller can merge it
    /// (H2/L1).
    pub async fn fetch_json(
        &self,
        key: &SecretKey,
    ) -> Result<Option<(JsonSyncDoc, i64)>, BridgeError> {
        let Some((doc, revision)) = self.fetch_json_unverified().await? else {
            return Ok(None);
        };
        doc.verify_integrity(key)?;
        doc.validate()?;
        Ok(Some((doc, revision)))
    }

    /// Pull + merge + push for the field-level encrypted document.
    /// `kdf`/`canary` of the local device win when present; otherwise the
    /// remote values are adopted (a device joining an existing vault).
    pub async fn sync_json(
        &self,
        key: &SecretKey,
        local: &Vault,
        kdf: Option<KdfParams>,
        canary: &str,
        device_id: &str,
        max_attempts: u32,
    ) -> Result<BridgeSyncOutcome, BridgeError> {
        // Local-only permissions are never part of a sync document, even if a
        // caller passes a vault that currently has them enabled (H2).
        let local_sanitized = local.without_remote_permissions();
        let local = &local_sanitized;
        local
            .validate_for_sync()
            .map_err(BridgeError::InvalidSyncDoc)?;
        let mut merged;
        let mut effective_kdf = kdf;
        let mut effective_canary = canary.to_string();
        // Re-evaluate on every retry: a 409 may be caused by another device
        // rotating the key, and stale KDF/canary must never be pushed.
        let starting_kdf = effective_kdf.clone();
        let starting_canary = effective_canary.clone();

        for attempt in 0..max_attempts.max(1) {
            merged = local.clone();
            let remote = self.fetch_json(key).await?;
            let (mut next_kdf, mut next_canary) =
                resolve_key_state(starting_kdf.as_ref(), &starting_canary, None, "")?;
            let base_revision = match &remote {
                Some((doc, revision)) => {
                    let (rk, rc) = resolve_key_state(
                        starting_kdf.as_ref(),
                        &starting_canary,
                        doc.kdf.as_ref(),
                        &doc.canary,
                    )?;
                    next_kdf = rk;
                    next_canary = rc;
                    // Integrity + structure were already checked by
                    // `fetch_json`; still scrub local-only flags before the
                    // merge so remote settings can never enable them (H2).
                    merged.merge(&doc.remote_vault());
                    merged.settings.chat_allow_credentials = local.settings.chat_allow_credentials;
                    merged.settings.chat_allow_vps = local.settings.chat_allow_vps;
                    *revision
                }
                None => 0,
            };
            effective_kdf = next_kdf;
            effective_canary = next_canary;

            let mut doc = JsonSyncDoc {
                schema: 1,
                revision: base_revision,
                device_id: device_id.to_string(),
                updated_at: Utc::now(),
                kdf: effective_kdf.clone(),
                canary: effective_canary.clone(),
                vault: merged.clone(),
                mac: String::new(),
            };
            doc.sign(key)?;
            let blob = serde_json::to_string(&doc)?;

            match self.push(base_revision, &blob).await {
                Ok(revision) => {
                    return Ok(BridgeSyncOutcome {
                        vault: merged,
                        revision,
                        pushed: true,
                        kdf: effective_kdf,
                        canary: effective_canary,
                    });
                }
                Err(BridgeError::Conflict) if attempt + 1 < max_attempts => continue,
                Err(e) => return Err(e),
            }
        }
        Err(BridgeError::TooManyRetries(max_attempts))
    }

    async fn push(&self, base_revision: i64, blob: &str) -> Result<i64, BridgeError> {
        let res = self
            .auth(self.http.put(format!("{}/v1/vault", self.base_url)))?
            .json(&PutBody {
                base_revision,
                blob,
            })
            .send()
            .await?;
        if res.status() == reqwest::StatusCode::CONFLICT {
            let _ = res.bytes().await; // conflict body (current blob) not needed; we re-pull
            return Err(BridgeError::Conflict);
        }
        let resp: PutResponse = parse(res).await?;
        Ok(resp.revision)
    }
}

pub struct BridgeSyncOutcome {
    pub vault: Vault,
    pub revision: i64,
    pub pushed: bool,
    pub kdf: Option<KdfParams>,
    pub canary: String,
}

async fn parse<T: for<'de> Deserialize<'de>>(res: reqwest::Response) -> Result<T, BridgeError> {
    if !res.status().is_success() {
        return Err(status_error(res).await);
    }
    Ok(res.json::<T>().await?)
}

async fn status_error(res: reqwest::Response) -> BridgeError {
    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();
    if status == reqwest::StatusCode::CONFLICT.as_u16() {
        return BridgeError::Conflict;
    }
    if status == reqwest::StatusCode::UNAUTHORIZED.as_u16() {
        return BridgeError::Unauthenticated;
    }
    BridgeError::Status { status, body }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diverging_kdf_fails_closed() {
        let a = KdfParams::new_default();
        let b = KdfParams::new_default();
        assert!(matches!(
            resolve_key_state(Some(&a), "canary", Some(&b), "canary"),
            Err(BridgeError::KeyMismatch)
        ));
        let (kdf, canary) = resolve_key_state(None, "", Some(&b), "remote").unwrap();
        assert_eq!(kdf, Some(b));
        assert_eq!(canary, "remote");
    }

    fn signed_doc(key: &SecretKey) -> JsonSyncDoc {
        let mut doc = JsonSyncDoc {
            schema: 1,
            revision: 4,
            device_id: "device-1".into(),
            updated_at: Utc::now(),
            kdf: None,
            canary: String::new(),
            vault: Vault::default(),
            mac: String::new(),
        };
        doc.sign(key).unwrap();
        doc
    }

    #[test]
    fn mac_covers_the_whole_envelope() {
        let key = SecretKey::generate();
        let doc = signed_doc(&key);
        doc.verify_integrity(&key).unwrap();

        let other = SecretKey::generate();
        assert!(matches!(
            doc.verify_integrity(&other),
            Err(BridgeError::InvalidMac)
        ));

        let mut changed = doc.clone();
        changed.revision += 1;
        assert!(matches!(
            changed.verify_integrity(&key),
            Err(BridgeError::InvalidMac)
        ));

        let mut changed = doc.clone();
        changed.vault.settings.font_size += 1;
        assert!(matches!(
            changed.verify_integrity(&key),
            Err(BridgeError::InvalidMac)
        ));

        let mut changed = doc;
        changed.vault.hosts.push(crate::model::Host {
            id: uuid::Uuid::new_v4(),
            label: String::new(),
            address: String::new(),
            port: 0,
            username: String::new(),
            auth: crate::model::AuthMethod::default(),
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
            terminal: crate::model::TerminalPrefs::default(),
            agent_forwarding: false,
            compression: false,
            os: None,
            host_key_fingerprint: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_used_at: None,
            deleted: true,
        });
        assert!(matches!(
            changed.verify_integrity(&key),
            Err(BridgeError::InvalidMac)
        ));
    }

    #[test]
    fn legacy_unsigned_doc_fails_closed() {
        let key = SecretKey::generate();
        let mut doc = signed_doc(&key);
        doc.mac.clear();
        assert!(matches!(
            doc.verify_integrity(&key),
            Err(BridgeError::MissingMac)
        ));
        doc.mac = "not-hex".into();
        assert!(matches!(
            doc.verify_integrity(&key),
            Err(BridgeError::InvalidMac)
        ));
    }

    #[test]
    fn remote_permissions_are_stripped_from_synced_documents() {
        let key = SecretKey::generate();
        let mut doc = signed_doc(&key);
        doc.vault.settings.chat_allow_credentials = true;
        doc.vault.settings.chat_allow_vps = true;
        doc.sign(&key).unwrap();
        doc.validate().unwrap();
        assert!(!doc.remote_vault().settings.chat_allow_credentials);
        assert!(!doc.remote_vault().settings.chat_allow_vps);
    }

    #[test]
    fn unsafe_remote_kdf_is_rejected() {
        let mut bad = KdfParams::new_default();
        bad.m_cost = u32::MAX;
        assert!(matches!(
            resolve_key_state(None, "", Some(&bad), "canary"),
            Err(BridgeError::UnsafeKdf)
        ));
    }
}
