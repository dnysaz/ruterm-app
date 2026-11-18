//! Sync through the user's own Google Drive `appDataFolder`.
//!
//! No rterm server is involved. Two payload shapes are supported:
//!
//! * [`DriveDoc`] — whole-file sealed envelope (legacy / defence in depth).
//! * [`JsonDriveDoc`] — the readable JSON document with **field-level**
//!   encrypted secrets (`enc:v1:…` strings). This is what the desktop app
//!   stores, so hosts/settings stay inspectable and diff-able while passwords
//!   and private keys are ciphertext.
//!
//! Concurrency is handled with an ETag compare-and-swap: pull, merge, write
//! with `If-Match`, retry on `412 Precondition Failed`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{open_vault, seal_vault, KdfParams, SecretKey, SealedVault};
use crate::model::Vault;

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
pub const DEFAULT_FILE_NAME: &str = "rterm-vault.json";

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("drive returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("google access token expired or invalid")]
    Unauthorized,
    #[error("revision conflict")]
    Conflict,
    #[error("vault encryption error: {0}")]
    Crypto(#[from] crate::crypto::CryptoError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sync did not converge after {0} attempts")]
    TooManyRetries(u32),
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    #[serde(default)]
    modified_time: Option<DateTime<Utc>>,
}

/// A resolved Drive file with the ETag we need for compare-and-swap.
#[derive(Clone, Debug)]
struct FileRef {
    id: String,
    etag: String,
    modified_at: Option<DateTime<Utc>>,
}

/// Whole-file sealed payload (legacy).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DriveDoc {
    pub schema: u32,
    pub revision: i64,
    pub device_id: String,
    pub updated_at: DateTime<Utc>,
    pub envelope: SealedVault,
}

/// Field-level encrypted payload: plain JSON, secrets already sealed.
///
/// `kdf` and `canary` are stored alongside the vault so a *new* device can
/// derive the same key from the same passphrase. Both are non-secret: the
/// salt only prevents precomputation, and the canary is ciphertext.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsonDriveDoc {
    pub schema: u32,
    pub revision: i64,
    pub device_id: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub kdf: Option<KdfParams>,
    #[serde(default)]
    pub canary: String,
    pub vault: Vault,
}

#[derive(Clone, Debug)]
pub struct RemoteDoc {
    pub doc: DriveDoc,
    pub etag: String,
    pub modified_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct RemoteJsonDoc {
    pub doc: JsonDriveDoc,
    pub etag: String,
    pub modified_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DriveSyncOutcome {
    pub vault: Vault,
    pub revision: i64,
    pub etag: String,
    pub device_id: String,
    pub pushed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct JsonSyncOutcome {
    pub vault: Vault,
    pub revision: i64,
    pub etag: String,
    pub pushed: bool,
    /// KDF actually used for this document (local one wins).
    pub kdf: Option<KdfParams>,
    pub canary: String,
}

pub struct DriveClient {
    http: reqwest::Client,
    token: String,
    file_name: String,
}

impl DriveClient {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .user_agent(concat!("rterm/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            token: access_token.into(),
            file_name: DEFAULT_FILE_NAME.to_string(),
        }
    }

    pub fn with_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = name.into();
        self
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    /// Finds the vault file (if any) and captures its ETag.
    async fn locate(&self) -> Result<Option<FileRef>, DriveError> {
        let query = format!(
            "name = '{}' and trashed = false",
            self.file_name.replace('\'', "")
        );
        let list: FileList = self
            .auth(self.http.get(format!("{DRIVE_API}/files")))
            .query(&[
                ("spaces", "appDataFolder"),
                ("q", query.as_str()),
                ("fields", "files(id,name,modifiedTime)"),
                ("pageSize", "10"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let Some(file) = list.files.into_iter().next() else {
            return Ok(None);
        };

        let meta_res = self
            .auth(self.http.get(format!("{DRIVE_API}/files/{}", file.id)))
            .query(&[("fields", "id,name,modifiedTime")])
            .send()
            .await?;
        if meta_res.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DriveError::Unauthorized);
        }
        let meta_res = meta_res.error_for_status()?;
        let etag = meta_res
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| DriveError::Status {
                status: 500,
                body: "drive response had no ETag".into(),
            })?;

        Ok(Some(FileRef {
            id: file.id,
            etag,
            modified_at: file.modified_time,
        }))
    }

    async fn download(&self, id: &str) -> Result<Vec<u8>, DriveError> {
        let res = self
            .auth(self.http.get(format!("{DRIVE_API}/files/{id}")))
            .query(&[("alt", "media")])
            .send()
            .await?;
        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DriveError::Unauthorized);
        }
        Ok(res.error_for_status()?.bytes().await?.to_vec())
    }

    /// Multipart create/replace. `current` = existing file for a CAS update.
    async fn upload(&self, data: &[u8], current: Option<&FileRef>) -> Result<String, DriveError> {
        let boundary = format!("rterm-{}", Uuid::new_v4().simple());
        let metadata = if current.is_some() {
            serde_json::json!({ "name": self.file_name })
        } else {
            serde_json::json!({
                "name": self.file_name,
                "parents": ["appDataFolder"],
                "mimeType": "application/json",
            })
        };
        let payload = std::str::from_utf8(data)
            .map_err(|e| DriveError::Status { status: 500, body: e.to_string() })?;
        let body = format!(
            "--{b}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{meta}\r\n\
             --{b}\r\nContent-Type: application/json\r\n\r\n{data}\r\n--{b}--",
            b = boundary,
            meta = metadata,
            data = payload
        );

        let url = match current {
            Some(file) => format!(
                "{UPLOAD_API}/files/{}?uploadType=multipart&fields=id,modifiedTime",
                file.id
            ),
            None => format!("{UPLOAD_API}/files?uploadType=multipart&fields=id,modifiedTime"),
        };

        let base = if current.is_some() {
            self.auth(self.http.patch(&url))
        } else {
            self.auth(self.http.post(&url))
        };
        let mut req = base
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body.to_string());
        if let Some(file) = current {
            req = req.header(reqwest::header::IF_MATCH, file.etag.as_str());
        }

        let res = req.send().await?;
        let status = res.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DriveError::Unauthorized);
        }
        if status == reqwest::StatusCode::PRECONDITION_FAILED {
            return Err(DriveError::Conflict);
        }
        if !status.is_success() {
            return Err(DriveError::Status {
                status: status.as_u16(),
                body: res.text().await.unwrap_or_default(),
            });
        }
        if let Some(etag) = res
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
        {
            return Ok(etag);
        }
        self.current_etag().await
    }

    /// Fresh ETag for the vault file (used after an upload).
    async fn current_etag(&self) -> Result<String, DriveError> {
        self.locate()
            .await?
            .map(|f| f.etag)
            .ok_or(DriveError::Status {
                status: 404,
                body: "vault file disappeared".into(),
            })
    }

    // ------------------------------------------------------------ envelope

    /// Downloads the current vault document, if the account has one.
    pub async fn fetch(&self) -> Result<Option<RemoteDoc>, DriveError> {
        let Some(file) = self.locate().await? else {
            return Ok(None);
        };
        let bytes = self.download(&file.id).await?;
        let doc: DriveDoc = serde_json::from_slice(&bytes)?;
        Ok(Some(RemoteDoc {
            doc,
            etag: file.etag,
            modified_at: file.modified_at,
        }))
    }

    /// Creates or replaces the sealed document (CAS when `etag` is `Some`).
    pub async fn store(&self, doc: &DriveDoc, etag: Option<&str>) -> Result<String, DriveError> {
        let data = serde_json::to_vec(doc)?;
        let current = match etag {
            Some(existing) => {
                let mut file = self.locate().await?.ok_or(DriveError::Status {
                    status: 404,
                    body: "vault file disappeared".into(),
                })?;
                file.etag = existing.to_string();
                Some(file)
            }
            None => None,
        };
        self.upload(&data, current.as_ref()).await
    }

    /// Pull + decrypt + merge + encrypt + push (whole-file envelope).
    pub async fn sync(
        &self,
        local: &Vault,
        key: &SecretKey,
        kdf: Option<KdfParams>,
        device_id: &str,
        max_attempts: u32,
    ) -> Result<DriveSyncOutcome, DriveError> {
        let mut merged = local.clone();
        for attempt in 0..max_attempts.max(1) {
            let remote = self.fetch().await?;
            let (revision, etag) = match &remote {
                Some(remote) => {
                    let remote_vault = open_vault(&remote.doc.envelope, key)?;
                    merged.merge(&remote_vault);
                    (remote.doc.revision, Some(remote.etag.clone()))
                }
                None => (0, None),
            };

            let doc = DriveDoc {
                schema: 1,
                revision: revision + 1,
                device_id: device_id.to_string(),
                updated_at: Utc::now(),
                envelope: seal_vault(&merged, key, kdf.clone())?,
            };

            match self.store(&doc, etag.as_deref()).await {
                Ok(new_etag) => {
                    return Ok(DriveSyncOutcome {
                        vault: merged,
                        revision: doc.revision,
                        etag: new_etag,
                        device_id: device_id.to_string(),
                        pushed: true,
                    });
                }
                Err(DriveError::Conflict) if attempt + 1 < max_attempts => continue,
                Err(e) => return Err(e),
            }
        }
        Err(DriveError::TooManyRetries(max_attempts))
    }

    // ---------------------------------------------------------- json (FE)

    /// Downloads the readable JSON document (secrets already sealed inside).
    pub async fn fetch_json(&self) -> Result<Option<RemoteJsonDoc>, DriveError> {
        let Some(file) = self.locate().await? else {
            return Ok(None);
        };
        let bytes = self.download(&file.id).await?;
        let doc: JsonDriveDoc = serde_json::from_slice(&bytes)?;
        Ok(Some(RemoteJsonDoc {
            doc,
            etag: file.etag,
            modified_at: file.modified_at,
        }))
    }

    /// Creates or replaces the JSON document (CAS when `etag` is `Some`).
    pub async fn store_json(
        &self,
        doc: &JsonDriveDoc,
        etag: Option<&str>,
    ) -> Result<String, DriveError> {
        let data = serde_json::to_vec_pretty(doc)?;
        let current = match etag {
            Some(existing) => {
                let mut file = self.locate().await?.ok_or(DriveError::Status {
                    status: 404,
                    body: "vault file disappeared".into(),
                })?;
                file.etag = existing.to_string();
                Some(file)
            }
            None => None,
        };
        self.upload(&data, current.as_ref()).await
    }

    /// Pull + merge + push for the field-level encrypted JSON document.
    ///
    /// `kdf`/`canary` of the local device win when present; otherwise the
    /// remote values are adopted (used when a device joins an existing vault).
    pub async fn sync_json(
        &self,
        local: &Vault,
        kdf: Option<KdfParams>,
        canary: &str,
        device_id: &str,
        max_attempts: u32,
    ) -> Result<JsonSyncOutcome, DriveError> {
        let mut merged = local.clone();
        let mut effective_kdf = kdf;
        let mut effective_canary = canary.to_string();
        for attempt in 0..max_attempts.max(1) {
            let remote = self.fetch_json().await?;
            let (revision, etag) = match &remote {
                Some(remote) => {
                    merged.merge(&remote.doc.vault);
                    if effective_kdf.is_none() {
                        effective_kdf = remote.doc.kdf.clone();
                    }
                    if effective_canary.is_empty() {
                        effective_canary = remote.doc.canary.clone();
                    }
                    (remote.doc.revision, Some(remote.etag.clone()))
                }
                None => (0, None),
            };

            let doc = JsonDriveDoc {
                schema: 1,
                revision: revision + 1,
                device_id: device_id.to_string(),
                updated_at: Utc::now(),
                kdf: effective_kdf.clone(),
                canary: effective_canary.clone(),
                vault: merged.clone(),
            };

            match self.store_json(&doc, etag.as_deref()).await {
                Ok(new_etag) => {
                    return Ok(JsonSyncOutcome {
                        vault: merged,
                        revision: doc.revision,
                        etag: new_etag,
                        pushed: true,
                        kdf: effective_kdf,
                        canary: effective_canary,
                    });
                }
                Err(DriveError::Conflict) if attempt + 1 < max_attempts => continue,
                Err(e) => return Err(e),
            }
        }
        Err(DriveError::TooManyRetries(max_attempts))
    }
}
