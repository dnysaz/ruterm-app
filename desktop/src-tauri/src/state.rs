//! Shared application state: unlocked vault, Google session, live SSH handles.

use rterm_core::crypto::{KdfParams, SecretKey};
use rterm_core::model::Vault;
use rterm_core::oauth::GoogleAccount;
use rterm_core::sftp::SftpSession;
use rterm_core::ssh::SshHandle;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::RwLock;

use crate::store::AppSettings;

const KEYRING_SERVICE: &str = "app.rterm";

#[derive(Clone)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub account: GoogleAccount,
    /// Unix seconds.
    pub expires_at: i64,
}

pub struct Paths {
    pub vault: PathBuf,
    pub settings: PathBuf,
}

/// One entry per live or recently finished session: a small rolling buffer of
/// raw terminal output so a webview reload can replay what xterm missed.
pub struct StreamEntry {
    pub buf: Vec<u8>,
    /// Absolute bytes appended over the session's lifetime.
    pub total: u64,
    pub closed: bool,
}

const OUTPUT_CAP: usize = 512 * 1024;

impl StreamEntry {
    pub fn new() -> Self {
        Self { buf: Vec::new(), total: 0, closed: false }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        self.total = self.total.saturating_add(bytes.len() as u64);
        if self.buf.len() > OUTPUT_CAP {
            let drop = self.buf.len() - OUTPUT_CAP;
            self.buf.drain(..drop);
        }
    }
}

pub struct AppState {
    pub paths: Paths,
    pub settings: AppSettings,
    pub settings_rw: RwLock<AppSettings>,
    pub key: RwLock<Option<SecretKey>>,
    pub kdf: RwLock<Option<KdfParams>>,
    /// Plaintext vault in memory (secrets decrypted).
    pub vault: RwLock<Option<Vault>>,
    pub revision: RwLock<i64>,
    pub session: RwLock<Option<Session>>,
    pub ssh: Mutex<HashMap<String, SshHandle>>,
    pub local: crate::local::LocalSessions,
    /// Live SFTP sessions, keyed by SFTP tab id. Tokio mutex: every SFTP
    /// command awaits the session, so the guard is held across await points.
    pub sftp: tokio::sync::Mutex<HashMap<String, SftpSession>>,
    /// Output buffers for every session, keyed by session id (tab id).
    pub output: Mutex<HashMap<String, StreamEntry>>,
}

impl AppState {
    pub fn new(paths: Paths, settings: AppSettings) -> Self {
        Self {
            paths,
            settings_rw: RwLock::new(settings.clone()),
            settings,
            key: RwLock::new(None),
            kdf: RwLock::new(None),
            vault: RwLock::new(None),
            revision: RwLock::new(0),
            session: RwLock::new(None),
            ssh: Mutex::new(HashMap::new()),
            local: Mutex::new(HashMap::new()),
            sftp: tokio::sync::Mutex::new(HashMap::new()),
            output: Mutex::new(HashMap::new()),
        }
    }

    pub async fn client_id(&self) -> Result<String, String> {
        let settings = self.settings_rw.read().await;
        if settings.google_client_id.trim().is_empty() {
            return Err("Google sign-in is not configured in this build".into());
        }
        Ok(settings.google_client_id.trim().to_string())
    }

    /// Base URL of the RuTerm bridge. Resolution order: `bridge_url` in
    /// settings.json (self-hosted), else the `RTERM_BRIDGE_URL` baked in at
    /// build time (official releases). Empty means sync is unavailable but
    /// the local vault keeps working.
    pub async fn bridge_url(&self) -> Result<String, String> {
        let settings = self.settings_rw.read().await;
        let local = settings.bridge_url.trim().to_string();
        if !local.is_empty() {
            return Ok(local);
        }
        if let Some(env_url) = option_env!("RTERM_BRIDGE_URL").map(str::trim).filter(|v| !v.is_empty()) {
            return Ok(env_url.to_string());
        }
        Err("Sync bridge is not configured yet — local vault still works offline".into())
    }

    pub async fn unlocked_key(&self) -> Result<SecretKey, String> {
        self.key
            .read()
            .await
            .clone()
            .ok_or_else(|| "Vault is locked".to_string())
    }

    pub async fn unlocked_vault(&self) -> Result<Vault, String> {
        self.vault
            .read()
            .await
            .clone()
            .ok_or_else(|| "Vault is locked".to_string())
    }
}

// --------------------------------------------------------------- keychain

pub fn keyring_set(account: &str, value: &str) -> bool {
    keyring::Entry::new(KEYRING_SERVICE, account)
        .and_then(|entry| entry.set_password(value))
        .is_ok()
}

pub fn keyring_get(account: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, account)
        .ok()
        .and_then(|entry| entry.get_password().ok())
}

pub fn keyring_delete(account: &str) {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, account) {
        let _ = entry.delete_credential();
    }
}

pub const KEY_VAULT: &str = "vault-key";
/// RuTerm bridge refresh token (rotated on every use).
pub const KEY_SYNC: &str = "sync-refresh";
/// Legacy Google Drive refresh token — deleted on sign-out; never used.
pub const KEY_OAUTH: &str = "oauth-refresh";
