//! Shared application state: unlocked vault, Google session, live SSH handles.

use rterm_core::crypto::{KdfParams, SecretKey};
use rterm_core::model::{Identity, Vault};
use rterm_core::oauth::GoogleAccount;
use rterm_core::sftp::SftpConnection;
use rterm_core::ssh::{SshHandle, SshTarget};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::RwLock;
use uuid::Uuid;

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
    /// Persisted trust-on-first-use host-key pins (`user@host:port` → SHA256).
    pub tofu: PathBuf,
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

/// A live SSH tab's recipe for reconnecting after an unexpected drop (e.g. a
/// VPS reboot). Kept in memory only; the vault stays the source of truth for
/// saved hosts, while quick-connect tabs reuse the cached one-shot target.
pub struct ReconnectEntry {
    pub target: SshTarget,
    /// `Some` for saved hosts so credentials can be refreshed from the vault.
    pub host_id: Option<String>,
    pub cols: u32,
    pub rows: u32,
    pub display_name: String,
    /// False once the user closes the tab; the supervisor checks this before
    /// every retry so a close always wins over an in-flight reconnect.
    pub enabled: bool,
    /// True while a reconnect attempt is in progress (drives the UI overlay).
    pub in_progress: bool,
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
    pub sftp: tokio::sync::Mutex<HashMap<String, SftpConnection>>,
    /// Output buffers for every session, keyed by session id (tab id).
    pub output: Mutex<HashMap<String, StreamEntry>>,
    /// Everything needed to reopen an SSH tab after the connection drops.
    /// Removed on a deliberate tab close so closing never reconnects.
    pub reconnect: Mutex<HashMap<String, ReconnectEntry>>,
    /// Opaque, Rust-issued capability per terminal session. Every write,
    /// resize, close and replay must present the token returned by `*_open`.
    pub session_tokens: Mutex<HashMap<String, String>>,
    /// Same capability model for SFTP sessions: every list/read/write/close
    /// must present the token returned by `sftp_open_*` (R-01).
    pub sftp_tokens: Mutex<HashMap<String, String>>,
    /// One-shot handles for local paths chosen in a native dialog. Renderer
    /// never supplies a raw local path to an SFTP command.
    pub local_file_tokens: Mutex<HashMap<String, PathBuf>>,
    /// Newly generated/imported private keys awaiting the renderer's next
    /// `vault_save`. Kept in Rust so the plaintext key never crosses IPC.
    pub pending_identities: Mutex<HashMap<Uuid, Identity>>,
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
            reconnect: Mutex::new(HashMap::new()),
            session_tokens: Mutex::new(HashMap::new()),
            sftp_tokens: Mutex::new(HashMap::new()),
            local_file_tokens: Mutex::new(HashMap::new()),
            pending_identities: Mutex::new(HashMap::new()),
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
    ///
    /// This value decides where OAuth codes, refresh tokens and the encrypted
    /// vault are sent, so it is validated before every use (F-02). Plain HTTP
    /// is only accepted for a loopback bridge in debug builds.
    pub async fn bridge_url(&self) -> Result<String, String> {
        let settings = self.settings_rw.read().await;
        let local = settings.bridge_url.trim();
        let candidate = if !local.is_empty() {
            local
        } else if let Some(env_url) =
            option_env!("RTERM_BRIDGE_URL").map(str::trim).filter(|v| !v.is_empty())
        {
            env_url
        } else {
            return Err("Sync bridge is not configured yet — local vault still works offline".into());
        };
        validate_bridge_url(candidate)
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

/// Rejects URLs that would leak OAuth material or the synced vault to a
/// plaintext or attacker-controlled endpoint.
pub fn validate_bridge_url(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        return Ok(String::new());
    }
    let url = tauri::Url::parse(value)
        .map_err(|_| "Bridge URL must be an absolute URL".to_string())?;
    match url.scheme() {
        "https" => {}
        "http" if cfg!(debug_assertions) => {
            let host = url.host_str().unwrap_or("");
            if host != "127.0.0.1" && host != "localhost" {
                return Err("A plaintext bridge URL is only allowed on loopback in debug builds".into());
            }
        }
        other => return Err(format!("Unsupported bridge URL scheme: {other}")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Bridge URL must not contain credentials".into());
    }
    Ok(value.trim_end_matches('/').to_string())
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
