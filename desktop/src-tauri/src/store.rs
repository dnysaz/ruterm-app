//! Local JSON storage.
//!
//! One readable document (`vault.json`) holds the whole user dataset; secrets
//! inside it are already sealed by [`crate::secrets`]. Writes are atomic
//! (temp file + fsync + rename) and keep a `.bak` copy of the previous state.

use chrono::{DateTime, Utc};
use rterm_core::crypto::KdfParams;
use rterm_core::model::Vault;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VaultFile {
    pub schema: u32,
    pub revision: i64,
    pub device_id: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub kdf: Option<KdfParams>,
    /// Sealed known string, used to verify a derived key.
    #[serde(default)]
    pub canary: String,
    pub vault: Vault,
}

impl VaultFile {
    pub fn new(device_id: &str, kdf: Option<KdfParams>, canary: String) -> Self {
        Self {
            schema: 1,
            revision: 0,
            device_id: device_id.to_string(),
            updated_at: Utc::now(),
            kdf,
            canary,
            vault: Vault::default(),
        }
    }
}

pub fn read(path: &Path) -> Result<Option<VaultFile>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("invalid vault.json: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("failed to read {}: {e}", path.display())),
    }
}

/// Atomic write: the old file is copied to `.bak`, the new one is flushed to
/// `.tmp` and renamed over the target, so a crash never leaves half a file.
pub fn write_atomic(path: &Path, file: &VaultFile) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    if path.exists() {
        let backup = path.with_extension("json.bak");
        let _ = fs::copy(path, backup);
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut out = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        out.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    Ok(())
}

/// Small key/value file for app preferences (client id, device id, …).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppSettings {
    #[serde(default = "default_client_id")]
    pub google_client_id: String,
    /// RuTerm bridge base URL (e.g. `https://sync.example.com`). Public
    /// endpoint, not a secret. Set once in this file (or bake in at build
    /// time with RTERM_BRIDGE_URL); there is intentionally no UI for it.
    #[serde(default)]
    pub bridge_url: String,
    #[serde(default)]
    pub device_id: String,
    /// Set once Google sign-in succeeds, so startup does not touch the OS
    /// keychain (and macOS does not prompt) before the user ever linked one.
    #[serde(default)]
    pub google_linked: bool,
    /// The vault key is cached in the OS keychain, so the unlock screen can
    /// offer "unlock with saved key" instead of prompting on every launch.
    #[serde(default)]
    pub key_saved: bool,
    /// Revision of the vault as of the last successful sync. The UI shows
    /// "synced" only when the local vault revision matches this.
    #[serde(default)]
    pub synced_revision: i64,
    /// RFC 3339 timestamp of the last successful sync, if any.
    #[serde(default)]
    pub last_synced_at: Option<String>,
}

pub fn default_client_id() -> String {
    // The Desktop OAuth client from the project's Google Cloud console.
    // Public identifier — PKCE, no secret. Shared by every platform build.
    "862686830243-0puhqofdgvdij8curlen9ic1odc2je7g.apps.googleusercontent.com".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            google_client_id: default_client_id(),
            bridge_url: String::new(),
            device_id: uuid::Uuid::new_v4().to_string(),
            google_linked: false,
            key_saved: false,
            synced_revision: 0,
            last_synced_at: None,
        }
    }
}

pub fn read_settings(path: &Path) -> AppSettings {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn write_settings(path: &Path, settings: &AppSettings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("failed to save settings: {e}"))
}
