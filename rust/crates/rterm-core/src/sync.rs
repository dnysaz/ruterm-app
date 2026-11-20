//! Sync client for the rterm sync server.
//!
//! The server is a dumb encrypted-blob store with revision numbers. Merge
//! happens here, on the device, using the decrypted vault.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::crypto::{open_vault, seal_vault, KdfParams, SecretKey, SealedVault};
use crate::model::Vault;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("revision conflict")]
    Conflict,
    #[error("not authenticated")]
    Unauthenticated,
    #[error(transparent)]
    Crypto(#[from] crate::crypto::CryptoError),
    #[error("merge did not converge after {0} attempts")]
    TooManyRetries(u32),
    #[error("oauth error: {0}")]
    OAuth(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub platform: String,
    #[serde(default)]
    pub app_version: String,
}

impl DeviceInfo {
    pub fn current() -> Self {
        Self {
            name: hostname(),
            platform: std::env::consts::OS.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "device".into())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct User {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AuthSession {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub user: User,
}

// F-12: bearer/refresh tokens must never appear in logs via `Debug`.
impl std::fmt::Debug for AuthSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthSession")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("user", &self.user)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub current: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RemoteVault {
    pub revision: i64,
    #[serde(default)]
    pub envelope: Option<SealedVault>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SyncOutcome {
    pub vault: Vault,
    pub revision: i64,
    pub fingerprint_of_remote: Option<DateTime<Utc>>,
    pub pushed: bool,
}

pub struct SyncClient {
    base_url: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl SyncClient {
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

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder, SyncError> {
        let token = self.token.as_ref().ok_or(SyncError::Unauthenticated)?;
        Ok(req.bearer_auth(token))
    }

    /// Exchange a Google ID token (from the native Google Sign-In SDK) for our
    /// own long-lived session.
    pub async fn login_google(
        &mut self,
        id_token: &str,
        device: DeviceInfo,
    ) -> Result<AuthSession, SyncError> {
        #[derive(Serialize)]
        struct Body<'a> {
            id_token: &'a str,
            device: DeviceInfo,
        }
        let res = self
            .http
            .post(format!("{}/v1/auth/google", self.base_url))
            .json(&Body { id_token, device })
            .send()
            .await?;
        let session: AuthSession = parse(res).await?;
        self.token = Some(session.access_token.clone());
        Ok(session)
    }

    pub async fn refresh(&mut self, refresh_token: &str) -> Result<AuthSession, SyncError> {
        #[derive(Serialize)]
        struct Body<'a> {
            refresh_token: &'a str,
        }
        let res = self
            .http
            .post(format!("{}/v1/auth/refresh", self.base_url))
            .json(&Body { refresh_token })
            .send()
            .await?;
        let session: AuthSession = parse(res).await?;
        self.token = Some(session.access_token.clone());
        Ok(session)
    }

    pub async fn me(&self) -> Result<User, SyncError> {
        let res = self.auth(self.http.get(format!("{}/v1/me", self.base_url)))?.send().await?;
        parse(res).await
    }

    pub async fn pull(&self) -> Result<RemoteVault, SyncError> {
        let res = self
            .auth(self.http.get(format!("{}/v1/vault", self.base_url)))?
            .send()
            .await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(RemoteVault {
                revision: 0,
                envelope: None,
                updated_at: None,
            });
        }
        parse(res).await
    }

    pub async fn push(&self, revision: i64, envelope: &SealedVault) -> Result<i64, SyncError> {
        #[derive(Serialize)]
        struct Body<'a> {
            revision: i64,
            envelope: &'a SealedVault,
        }
        #[derive(Deserialize)]
        struct Resp {
            revision: i64,
        }
        let res = self
            .auth(self.http.put(format!("{}/v1/vault", self.base_url)))?
            .json(&Body { revision, envelope })
            .send()
            .await?;
        if res.status() == reqwest::StatusCode::CONFLICT {
            return Err(SyncError::Conflict);
        }
        let resp: Resp = parse(res).await?;
        Ok(resp.revision)
    }

    pub async fn devices(&self) -> Result<Vec<DeviceRow>, SyncError> {
        let res = self
            .auth(self.http.get(format!("{}/v1/devices", self.base_url)))?
            .send()
            .await?;
        parse(res).await
    }

    pub async fn revoke_device(&self, id: &str) -> Result<(), SyncError> {
        let res = self
            .auth(self.http.delete(format!("{}/v1/devices/{id}", self.base_url)))?
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(status_error(res).await);
        }
        Ok(())
    }
}

async fn parse<T: for<'de> Deserialize<'de>>(res: reqwest::Response) -> Result<T, SyncError> {
    if !res.status().is_success() {
        return Err(status_error(res).await);
    }
    Ok(res.json::<T>().await?)
}

async fn status_error(res: reqwest::Response) -> SyncError {
    let status = res.status().as_u16();
    let body = res.text().await.unwrap_or_default();
    SyncError::Status { status, body }
}

/// Full round trip: pull, decrypt, merge, re-encrypt, push. Retries on
/// revision conflicts (another device wrote in the meantime).
pub async fn sync_vault(
    client: &SyncClient,
    local: &Vault,
    key: &SecretKey,
    kdf: Option<KdfParams>,
    max_attempts: u32,
) -> Result<SyncOutcome, SyncError> {
    let mut merged = local.clone();

    for attempt in 0..max_attempts.max(1) {
        let remote = client.pull().await?;
        let revision = remote.revision;
        if let Some(envelope) = &remote.envelope {
            let remote_vault = open_vault(envelope, key)?;
            merged.merge(&remote_vault);
        }
        let remote_updated_at = remote.updated_at;

        let envelope = seal_vault(&merged, key, kdf.clone())?;
        match client.push(revision, &envelope).await {
            Ok(new_revision) => {
                return Ok(SyncOutcome {
                    vault: merged,
                    revision: new_revision,
                    fingerprint_of_remote: remote_updated_at,
                    pushed: true,
                });
            }
            Err(SyncError::Conflict) if attempt + 1 < max_attempts => continue,
            Err(e) => return Err(e),
        }
    }
    Err(SyncError::TooManyRetries(max_attempts))
}
