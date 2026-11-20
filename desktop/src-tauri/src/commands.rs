//! Tauri commands: Google sign-in, vault lifecycle, bridge sync, SSH sessions.

use base64::Engine;
use chrono::Utc;
use rterm_core::bridge::{BridgeClient, DeviceInfo};
use rterm_core::crypto::{derive_key, KdfParams};
use rterm_core::model::{AuthMethod, Identity, IdentityKind, Vault};
use rterm_core::oauth::{self, GoogleAccount};
use rterm_core::sftp;
use rterm_core::ssh::{self, SshEvent, SshTarget, TermSize};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Semaphore;
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use crate::secrets;
use crate::state::{
    keyring_delete, keyring_get, keyring_set, AppState, Session, StreamEntry,
    KEY_OAUTH, KEY_SYNC, KEY_VAULT,
};
use crate::store::{self, AppSettings, VaultFile};

const CANARY: &str = "rterm-canary-v1";
const SESSION_TTL_SKEW_SECS: i64 = 60;

/// Explicit opt-in for storing the vault key in the OS keychain. Nothing is
/// written implicitly on create/unlock/import (F-31).
#[tauri::command]
pub async fn vault_remember_key(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        let key = state
            .key
            .read()
            .await
            .clone()
            .ok_or("Unlock the vault before saving its key")?;
        // F-31: zeroize the intermediate base64 string, and only claim the
        // setting is saved when the keychain write actually succeeded.
        let encoded = zeroize::Zeroizing::new(key.to_base64());
        if !keyring_set(KEY_VAULT, &encoded) {
            return Err("Could not save the key to the OS keychain".into());
        }
    } else if keyring_get(KEY_VAULT).is_some() {
        keyring_delete(KEY_VAULT);
        if keyring_get(KEY_VAULT).is_some() {
            return Err("Could not remove the saved key from the OS keychain".into());
        }
    }
    let mut settings = state.settings_rw.write().await;
    settings.key_saved = enabled;
    store::write_settings(&state.paths.settings, &settings)
}

fn now_secs() -> i64 {
    Utc::now().timestamp()
}

/// Stable identity for an adhoc host's TOFU pin (F-46).
fn tofu_host_key(username: &str, address: &str, port: u16) -> String {
    format!("{}@{}:{}", username.trim(), address.trim(), port)
}

/// Resolves the host-key policy for a target: a previously persisted pin
/// becomes a strict `Pinned` check; a new host uses TOFU for this first contact.
fn tofu_policy(state: &AppState, username: &str, address: &str, port: u16) -> rterm_core::ssh::HostKeyPolicy {
    let pins = store::read_tofu(&state.paths.tofu);
    match pins.get(&tofu_host_key(username, address, port)) {
        Some(fp) if !fp.is_empty() => rterm_core::ssh::HostKeyPolicy::Pinned { fingerprint: fp.clone() },
        _ => rterm_core::ssh::HostKeyPolicy::TrustOnFirstUse,
    }
}

/// Remembers a fingerprint observed on a first TOFU connection. A changed pin
/// is an error (surfaced to the user), never silently replaced.
fn persist_tofu(
    state: &AppState,
    host_key: &str,
    policy: &rterm_core::ssh::HostKeyPolicy,
    fingerprint: &str,
) -> Result<(), String> {
    if matches!(policy, rterm_core::ssh::HostKeyPolicy::TrustOnFirstUse)
        && !host_key.is_empty()
        && !fingerprint.is_empty()
    {
        // F-46: fail closed — an unpersisted pin means the next connection
        // would silently re-trust any key, so surface the failure instead of
        // discarding it. A changed pin is refused by `remember_fingerprint`
        // and now aborts the connection with a clear error.
        store::remember_fingerprint(&state.paths.tofu, host_key, fingerprint)
            .map_err(|e| format!("Could not save host-key pin: {e}"))?;
    }
    Ok(())
}

/// Opens external links from Rust so the URL allowlist cannot be bypassed by
/// injected renderer code calling the opener plugin directly (F-32).
#[tauri::command]
pub fn open_external_link(url: String) -> Result<(), String> {
    let value = url.trim();
    if value.is_empty() || value.len() > 2048 {
        return Err("invalid link".into());
    }
    // F-32: reject control characters / whitespace for *every* scheme,
    // including mailto: and tel: (previously they skipped all validation).
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("invalid link".into());
    }
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("mailto:") {
        let body = &value[7..];
        if body.is_empty() || body.starts_with('/') || body.contains('\\') {
            return Err("invalid mail link".into());
        }
        tauri_plugin_opener::open_url(value, None::<&str>)
            .map_err(|e| format!("cannot open link: {e}"))?;
        return Ok(());
    }
    if lower.starts_with("tel:") {
        let body = &value[4..];
        if body.is_empty() || body.starts_with('/') || !body.chars().all(|c| c.is_ascii_digit() || "+-*#(),; ".contains(c)) {
            return Err("invalid tel link".into());
        }
        tauri_plugin_opener::open_url(value, None::<&str>)
            .map_err(|e| format!("cannot open link: {e}"))?;
        return Ok(());
    }
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("unsupported link scheme".into());
    }
    let rest = &value[value.find("://").map(|i| i + 3).unwrap_or(0)..];
    if rest.contains('@') || rest.contains('\\') || rest.chars().any(char::is_whitespace) {
        return Err("invalid link".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("invalid link".into());
    }
    tauri_plugin_opener::open_url(value, None::<&str>)
        .map_err(|e| format!("cannot open link: {e}"))?;
    Ok(())
}

#[derive(Serialize)]
pub struct AppStatus {
    pub has_vault: bool,
    pub unlocked: bool,
    pub signed_in: bool,
    pub account: Option<GoogleAccount>,
    pub device_id: String,
    pub revision: i64,
    pub vault_path: String,
    pub key_saved: bool,
    /// Revision as of the last successful sync (persisted in settings.json).
    pub synced_revision: i64,
    /// RFC 3339 timestamp of the last successful sync, or null if never.
    pub last_synced_at: Option<String>,
}

#[derive(Serialize)]
pub struct VaultPayload {
    pub vault: Vault,
    pub revision: i64,
}

/// Placeholder sent to the renderer in place of a real secret. The renderer
/// can echo it back unchanged when saving a host; Rust restores the real
/// value from its own unlocked state, so no private key or password ever has
/// to cross the IPC boundary in plaintext.
pub const SEALED_PLACEHOLDER: &str = "__RTERM_SEALED__";

fn is_sealed(value: &str) -> bool {
    value == SEALED_PLACEHOLDER
}

fn redact_identity(identity: &Identity) -> Identity {
    let mut out = identity.clone();
    if !out.private_key.is_empty() {
        out.private_key = SEALED_PLACEHOLDER.to_string();
    }
    if out.passphrase.as_deref().is_some_and(|p| !p.is_empty()) {
        out.passphrase = Some(SEALED_PLACEHOLDER.to_string());
    }
    out
}

fn redact_vault(vault: &Vault) -> Vault {
    let mut out = vault.clone();
    for host in &mut out.hosts {
        host.auth = match &host.auth {
            AuthMethod::Password { password } if !password.is_empty() => {
                AuthMethod::Password { password: SEALED_PLACEHOLDER.to_string() }
            }
            AuthMethod::KeyboardInteractive { password } if !password.is_empty() => {
                AuthMethod::KeyboardInteractive { password: SEALED_PLACEHOLDER.to_string() }
            }
            AuthMethod::Key { identity_id, passphrase }
                if passphrase.as_deref().is_some_and(|p| !p.is_empty()) =>
            {
                AuthMethod::Key {
                    identity_id: *identity_id,
                    passphrase: Some(SEALED_PLACEHOLDER.to_string()),
                }
            }
            other => other.clone(),
        };
    }
    out.identities = out.identities.iter().map(redact_identity).collect();
    out
}

/// Every command that hands a vault to the renderer goes through here.
fn sealed_payload(vault: &Vault, revision: i64) -> VaultPayload {
    VaultPayload {
        vault: redact_vault(vault),
        revision,
    }
}

// ------------------------------------------------------- session capabilities

/// Session ids double as Tauri event channels, so they must not be attacker-
/// chosen strings. The UI still creates them early (to subscribe before the
/// first byte), but Rust owns the capability token for every operation.
fn checked_session_id(session_id: &str) -> Result<String, String> {
    let id = session_id.trim();
    Uuid::parse_str(id).map_err(|_| "Invalid session id".to_string())?;
    Ok(id.to_string())
}

fn issue_session_token(state: &AppState, session_id: &str) -> String {
    let token = Uuid::new_v4().to_string();
    state
        .session_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(session_id.to_string(), token.clone());
    token
}

fn require_session_token(
    state: &AppState,
    session_id: &str,
    token: &str,
) -> Result<String, String> {
    let id = checked_session_id(session_id)?;
    let map = state
        .session_tokens
        .lock()
        .map_err(|_| "state lock".to_string())?;
    match map.get(&id) {
        Some(expected) if expected == token => Ok(id),
        _ => Err("Invalid or expired session token".into()),
    }
}

// R-01: SFTP sessions carry the same Rust-issued capability as PTY/local
// sessions. Knowing an sftp tab id (an event channel) is not enough to drive
// another tab's session.
fn issue_sftp_token(state: &AppState, sftp_id: &str) -> String {
    let token = Uuid::new_v4().to_string();
    state
        .sftp_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(sftp_id.to_string(), token.clone());
    token
}

fn require_sftp_token(state: &AppState, sftp_id: &str, token: &str) -> Result<String, String> {
    let id = checked_session_id(sftp_id)?;
    let map = state
        .sftp_tokens
        .lock()
        .map_err(|_| "state lock".to_string())?;
    match map.get(&id) {
        Some(expected) if expected == token => Ok(id),
        _ => Err("Invalid or expired SFTP token".into()),
    }
}

/// Structural checks before a renderer-supplied vault touches disk or memory.
/// The renderer is untrusted: duplicate ids, dangling key references and bad
/// schemas would otherwise corrupt sync or silently rebind credentials (F-27).
fn validate_vault(vault: &Vault) -> Result<(), String> {
    if vault.schema != 1 {
        return Err(format!("Unsupported vault schema {}", vault.schema));
    }
    let mut ids = HashSet::new();
    let mut unique = |id: Uuid, label: &str| -> Result<(), String> {
        if !ids.insert(id) {
            return Err(format!("Duplicate {label} id {id}"));
        }
        Ok(())
    };

    for identity in &vault.identities {
        unique(identity.id, "identity")?;
        if identity.label.trim().is_empty() && !identity.deleted {
            return Err("An identity needs a label".into());
        }
    }
    for host in &vault.hosts {
        unique(host.id, "host")?;
        if host.address.trim().is_empty() && !host.deleted {
            return Err("A host needs an address".into());
        }
        if host.port == 0 && !host.deleted {
            return Err("A host needs a valid port".into());
        }
        if let AuthMethod::Key { identity_id, .. } = &host.auth {
            if !host.deleted && !vault.identities.iter().any(|i| i.id == *identity_id && !i.deleted) {
                return Err("Host references a missing identity".into());
            }
        }
    }
    for note in &vault.notes {
        unique(note.id, "note")?;
    }
    for group in &vault.groups {
        unique(group.id, "group")?;
    }
    for snippet in &vault.snippets {
        unique(snippet.id, "snippet")?;
    }
    for forward in &vault.port_forwards {
        unique(forward.id, "port forward")?;
    }
    Ok(())
}

/// Rehydrate secrets that the renderer echoed back as `SEALED_PLACEHOLDER`.
/// An empty string is a deliberate clear; a missing host/key yields an empty
/// value so an unknown id can never inherit another entry's credential.
async fn restore_secrets(state: &AppState, incoming: &mut Vault) {
    let current = state.vault.read().await.clone();

    for host in &mut incoming.hosts {
        let old = current
            .as_ref()
            .and_then(|v| v.hosts.iter().find(|h| h.id == host.id))
            .map(|h| &h.auth);
        match (&mut host.auth, old) {
            (AuthMethod::Password { password }, Some(AuthMethod::Password { password: old }))
                if is_sealed(password) => *password = old.clone(),
            (AuthMethod::Password { password }, None) if is_sealed(password) => password.clear(),
            (
                AuthMethod::KeyboardInteractive { password },
                Some(AuthMethod::KeyboardInteractive { password: old }),
            ) if is_sealed(password) => *password = old.clone(),
            (AuthMethod::KeyboardInteractive { password }, None) if is_sealed(password) => {
                password.clear()
            }
            (
                AuthMethod::Key { passphrase, .. },
                Some(AuthMethod::Key { passphrase: old, .. }),
            ) if passphrase.as_deref().is_some_and(is_sealed) => *passphrase = old.clone(),
            (AuthMethod::Key { passphrase, .. }, None)
                if passphrase.as_deref().is_some_and(is_sealed) =>
            {
                *passphrase = None
            }
            _ => {}
        }
    }

    let pending = state
        .pending_identities
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for identity in &mut incoming.identities {
        let old = current
            .as_ref()
            .and_then(|v| v.identities.iter().find(|i| i.id == identity.id))
            .or_else(|| pending.get(&identity.id));
        if is_sealed(&identity.private_key) {
            identity.private_key = old.map(|i| i.private_key.clone()).unwrap_or_default();
        }
        if identity.passphrase.as_deref().is_some_and(is_sealed) {
            identity.passphrase = old.and_then(|i| i.passphrase.clone());
        }
    }
}

#[tauri::command]
pub async fn app_status(state: tauri::State<'_, AppState>) -> Result<AppStatus, String> {
    let settings = state.settings_rw.read().await.clone();
    let session = state.session.read().await;
    Ok(AppStatus {
        has_vault: state.paths.vault.exists(),
        unlocked: state.vault.read().await.is_some(),
        signed_in: session.is_some(),
        account: session.as_ref().map(|s| s.account.clone()),
        device_id: settings.device_id,
        revision: *state.revision.read().await,
        vault_path: state.paths.vault.display().to_string(),
        key_saved: settings.key_saved,
        synced_revision: settings.synced_revision,
        last_synced_at: settings.last_synced_at.clone(),
    })
}

// ------------------------------------------------------------------- auth

#[tauri::command]
pub async fn auth_sign_in(state: tauri::State<'_, AppState>) -> Result<GoogleAccount, String> {
    let client_id = state.client_id().await?;
    let bridge_url = state.bridge_url().await?;
    let flow = oauth::authorize_loopback(&client_id, oauth::IDENTITY_SCOPES, Duration::from_secs(300))
        .await
        .map_err(|e| e.to_string())?;

    let session = BridgeClient::new(&bridge_url)
        .auth_google(&flow.code, &flow.code_verifier, &flow.redirect_uri, DeviceInfo::current())
        .await
        .map_err(|e| e.to_string())?;

    let account = GoogleAccount {
        sub: session.user.sub.clone().unwrap_or_default(),
        email: session.user.email.clone(),
        name: Some(session.user.name).filter(|n| !n.is_empty()),
        picture: session.user.picture.clone().filter(|p| !p.is_empty()),
    };

    if keyring_set(KEY_SYNC, &session.refresh_token) {
        let _ = keyring_set(
            "account",
            &serde_json::to_string(&account).unwrap_or_default(),
        );
    }
    // Drop any legacy Drive session from pre-bridge builds.
    keyring_delete(KEY_OAUTH);

    {
        let mut settings = state.settings_rw.write().await;
        settings.google_linked = true;
        store::write_settings(&state.paths.settings, &settings)?;
    }

    *state.session.write().await = Some(Session {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        account: account.clone(),
        expires_at: now_secs() + session.expires_in as i64,
    });
    Ok(account)
}

/// Silent sign-in using the bridge refresh token in the OS keychain.
#[tauri::command]
pub async fn auth_restore(state: tauri::State<'_, AppState>) -> Result<Option<GoogleAccount>, String> {
    if !state.settings_rw.read().await.google_linked {
        return Ok(None);
    }
    let Some(refresh) = keyring_get(KEY_SYNC) else {
        return Ok(None);
    };
    let bridge_url = match state.bridge_url().await {
        Ok(url) => url,
        Err(_) => return Ok(None),
    };
    let session = match BridgeClient::new(&bridge_url).refresh(&refresh).await {
        Ok(s) => s,
        Err(_) => {
            // Stale/revoked token: forget it, user signs in again.
            keyring_delete(KEY_SYNC);
            return Ok(None);
        }
    };
    keyring_set(KEY_SYNC, &session.refresh_token);
    // Bridge older than the picture/sub rollout returns neither — merge with
    // the cached account so the photo doesn't vanish across restarts.
    let cached: Option<GoogleAccount> = keyring_get("account")
        .and_then(|raw| serde_json::from_str(&raw).ok());
    let account: GoogleAccount = {
        let fresh_name = Some(session.user.name.clone()).filter(|n| !n.is_empty());
        let fresh_picture = session.user.picture.clone().filter(|p| !p.is_empty());
        let fresh_sub = session.user.sub.clone().unwrap_or_default();
        GoogleAccount {
            sub: if fresh_sub.is_empty() {
                cached.as_ref().map(|c| c.sub.clone()).unwrap_or_default()
            } else {
                fresh_sub
            },
            email: session.user.email.clone(),
            name: fresh_name.or_else(|| cached.as_ref().and_then(|c| c.name.clone())),
            picture: fresh_picture.or_else(|| cached.as_ref().and_then(|c| c.picture.clone())),
        }
    };
    let _ = keyring_set("account", &serde_json::to_string(&account).unwrap_or_default());
    *state.session.write().await = Some(Session {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        account: account.clone(),
        expires_at: now_secs() + session.expires_in as i64,
    });
    Ok(Some(account))
}

#[tauri::command]
pub async fn auth_sign_out(state: tauri::State<'_, AppState>) -> Result<(), String> {
    // Best effort: revoke the refresh token server-side too.
    if let Some(session) = state.session.read().await.clone() {
        if let Ok(url) = state.bridge_url().await {
            let mut client = BridgeClient::new(&url);
            client.set_token(Some(session.access_token.clone()));
            let _ = client.logout(&session.refresh_token).await;
        }
    }
    keyring_delete(KEY_SYNC);
    keyring_delete(KEY_OAUTH);
    keyring_delete("account");
    *state.session.write().await = None;
    let mut settings = state.settings_rw.write().await;
    settings.google_linked = false;
    store::write_settings(&state.paths.settings, &settings)?;
    Ok(())
}

async fn ensure_access_token(state: &AppState) -> Result<String, String> {
    let mut guard = state.session.write().await;
    let Some(session) = guard.as_mut() else {
        return Err("Not signed in".into());
    };
    if session.expires_at > now_secs() + SESSION_TTL_SKEW_SECS {
        return Ok(session.access_token.clone());
    }
    let bridge_url = state.bridge_url().await?;
    let refreshed = BridgeClient::new(&bridge_url)
        .refresh(&session.refresh_token)
        .await
        .map_err(|e| e.to_string())?;
    session.access_token = refreshed.access_token;
    if !refreshed.refresh_token.is_empty() {
        session.refresh_token = refreshed.refresh_token.clone();
        keyring_set(KEY_SYNC, &refreshed.refresh_token);
    }
    session.expires_at = now_secs() + refreshed.expires_in as i64;
    Ok(session.access_token.clone())
}

// ------------------------------------------------------------------ vault

#[tauri::command]
pub async fn vault_create(
    state: tauri::State<'_, AppState>,
    passphrase: String,
) -> Result<VaultPayload, String> {
    if passphrase.chars().count() < 8 {
        return Err("Use at least 8 characters".into());
    }
    if state.paths.vault.exists() {
        return Err("A vault already exists — unlock it with your passphrase".into());
    }
    let kdf = KdfParams::new_default();
    let key = derive_key(&passphrase, &kdf).map_err(|e| e.to_string())?;
    let canary = secrets::seal_canary(&key, CANARY).map_err(|e| e.to_string())?;

    let vault = Vault::default();
    let mut file = VaultFile::new(&state.settings.device_id, Some(kdf.clone()), canary);
    file.vault = secrets::seal_vault(&vault, &key, &secrets::kdf_epoch(Some(&kdf)))
        .map_err(|e| e.to_string())?;
    store::write_atomic(&state.paths.vault, &file)?;

    *state.key.write().await = Some(key);
    *state.kdf.write().await = Some(kdf);
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(sealed_payload(&vault, file.revision))
}

/// Last-resort recovery: discard the unreadable local vault and start over.
/// The confirmation string is checked here too, not only in the UI.
#[tauri::command]
pub async fn vault_reset(
    state: tauri::State<'_, AppState>,
    passphrase: String,
    confirmation: String,
) -> Result<VaultPayload, String> {
    if confirmation != "RESET" {
        return Err("Type RESET to confirm the vault reset".into());
    }
    if passphrase.chars().count() < 8 {
        return Err("Use at least 8 characters".into());
    }
    if state.key.read().await.is_some() {
        return Err("Lock the vault before resetting it".into());
    }

    let kdf = KdfParams::new_default();
    let key = derive_key(&passphrase, &kdf).map_err(|e| e.to_string())?;
    let canary = secrets::seal_canary(&key, CANARY).map_err(|e| e.to_string())?;
    let vault = Vault::default();
    let mut file = VaultFile::new(&state.settings.device_id, Some(kdf.clone()), canary);
    file.vault = secrets::seal_vault(&vault, &key, &secrets::kdf_epoch(Some(&kdf)))
        .map_err(|e| e.to_string())?;
    store::write_atomic(&state.paths.vault, &file)?;

    // A stale saved key would keep pointing at the discarded vault.
    keyring_delete(KEY_VAULT);
    let mut settings = state.settings_rw.write().await;
    settings.key_saved = false;
    settings.synced_revision = 0;
    settings.last_synced_at = None;
    store::write_settings(&state.paths.settings, &settings)?;
    drop(settings);

    *state.key.write().await = Some(key);
    *state.kdf.write().await = Some(kdf);
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(sealed_payload(&vault, file.revision))
}

#[tauri::command]
pub async fn vault_unlock(
    state: tauri::State<'_, AppState>,
    passphrase: String,
) -> Result<VaultPayload, String> {
    let file = store::read(&state.paths.vault)?.ok_or("No vault on this device yet")?;
    let kdf = file
        .kdf
        .clone()
        .ok_or("vault.json has no KDF parameters")?;
    let key = derive_key(&passphrase, &kdf).map_err(|_| "Wrong passphrase".to_string())?;
    unlock_with_key(&state, &file, key).await
}

/// Joins an existing vault stored on the bridge.
///
/// Needed on a fresh device: the KDF salt rides in the (field-encrypted)
/// vault document, so the same passphrase yields the same key there.
#[tauri::command]
pub async fn vault_adopt_remote(
    state: tauri::State<'_, AppState>,
    passphrase: String,
) -> Result<VaultPayload, String> {
    let token = ensure_access_token(&state).await?;
    let bridge_url = state.bridge_url().await?;
    let mut client = BridgeClient::new(&bridge_url);
    client.set_token(Some(token));
    let (doc, revision) = client
        .fetch_json()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("No vault found for this account yet")?;

    let kdf = doc
        .kdf
        .clone()
        .ok_or("The synced vault has no KDF parameters")?;
    let key = derive_key(&passphrase, &kdf).map_err(|e| e.to_string())?;
    if !secrets::verify_canary(&key, &doc.canary, CANARY) {
        return Err("Wrong passphrase".into());
    }
    let epoch = secrets::kdf_epoch(Some(&kdf));
    let (vault, _) =
        secrets::open_vault_with_context(&doc.vault, &key, &epoch).map_err(|e| e.to_string())?;
    // R-13: a remote vault is untrusted input too — apply the same structural
    // checks before it touches disk or memory.
    validate_vault(&vault)?;

    // Re-seal with the current field-bound AAD. An old bridge copy may still
    // hold pre-hardening ciphertext; this migrates it on adoption.
    let canary = secrets::seal_canary(&key, CANARY).map_err(|e| e.to_string())?;
    let sealed_vault = secrets::seal_vault(&vault, &key, &epoch).map_err(|e| e.to_string())?;
    let mut file = VaultFile::new(&state.settings.device_id, Some(kdf.clone()), canary);
    file.revision = revision;
    file.updated_at = Utc::now();
    file.vault = sealed_vault;
    store::write_atomic(&state.paths.vault, &file)?;

    *state.key.write().await = Some(key);
    *state.kdf.write().await = Some(kdf);
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    mark_synced(&state, file.revision).await;
    Ok(sealed_payload(&vault, file.revision))
}

/// Authoritative check of whether a vault key actually exists in the keychain.
/// Unlike `vault_try_auto_unlock`, this never decrypts and never deletes, so the
/// UI can tell "no saved key" apart from "saved key, transient unlock failure"
/// without destroying the key (F-31).
#[tauri::command]
pub fn vault_saved_key_exists() -> bool {
    keyring_get(KEY_VAULT).is_some()
}

/// Unlock using the key cached in the OS keychain (no passphrase prompt).
#[tauri::command]
pub async fn vault_try_auto_unlock(
    state: tauri::State<'_, AppState>,
) -> Result<Option<VaultPayload>, String> {
    let Some(file) = store::read(&state.paths.vault)? else {
        return Ok(None);
    };
    let Some(raw_key) = keyring_get(KEY_VAULT) else {
        return Ok(None);
    };
    let raw_key = zeroize::Zeroizing::new(raw_key);
    let Ok(key) = rterm_core::crypto::SecretKey::from_base64(&raw_key) else {
        // Corrupt entry or a transient keychain failure: leave it in place.
        // Only an explicit "forget saved key" may delete it (F-31).
        return Ok(None);
    };
    unlock_with_key(&state, &file, key).await.map(Some)
}

async fn unlock_with_key(
    state: &AppState,
    file: &VaultFile,
    key: rterm_core::crypto::SecretKey,
) -> Result<VaultPayload, String> {
    if !secrets::verify_canary(&key, &file.canary, CANARY) {
        return Err("Wrong passphrase".into());
    }
    let epoch = secrets::kdf_epoch(file.kdf.as_ref());
    let (vault, legacy_vault) =
        secrets::open_vault_with_context(&file.vault, &key, &epoch).map_err(|e| e.to_string())?;

    // Vaults written before the AAD hardening kept the same `enc:v1:` prefix.
    // The passphrase was never wrong — the AEAD context was. Re-seal the local
    // copy once so future opens use the strict, field-bound context.
    if legacy_vault || secrets::canary_is_legacy(&key, &file.canary, CANARY) {
        let mut migrated = file.clone();
        migrated.canary = secrets::seal_canary(&key, CANARY).map_err(|e| e.to_string())?;
        migrated.vault = secrets::seal_vault(&vault, &key, &epoch).map_err(|e| e.to_string())?;
        store::write_atomic(&state.paths.vault, &migrated)?;
    }

    *state.key.write().await = Some(key);
    *state.kdf.write().await = file.kdf.clone();
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(sealed_payload(&vault, file.revision))
}

#[tauri::command]
pub async fn vault_lock(state: tauri::State<'_, AppState>) -> Result<(), String> {
    *state.key.write().await = None;
    *state.kdf.write().await = None;
    *state.vault.write().await = None;
    if let Ok(mut tokens) = state.session_tokens.lock() {
        tokens.clear();
    }
    if let Ok(mut tokens) = state.sftp_tokens.lock() {
        tokens.clear();
    }
    state
        .pending_identities
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    Ok(())
}

#[tauri::command]
pub async fn vault_get(state: tauri::State<'_, AppState>) -> Result<Option<VaultPayload>, String> {
    let Some(vault) = state.vault.read().await.clone() else {
        return Ok(None);
    };
    Ok(Some(sealed_payload(&vault, *state.revision.read().await)))
}

/// Remember that the local vault is in sync with the bridge at `revision`.
async fn mark_synced(state: &AppState, revision: i64) {
    let mut settings = state.settings_rw.write().await;
    settings.synced_revision = revision;
    settings.last_synced_at = Some(Utc::now().to_rfc3339());
    let _ = store::write_settings(&state.paths.settings, &settings);
}

/// Seals `vault` into the on-disk file and keeps in-memory state in sync.
async fn persist_vault(state: &AppState, mut vault: Vault) -> Result<VaultPayload, String> {
    validate_vault(&vault)?;
    let key = state.unlocked_key().await?;
    vault.updated_at = Some(Utc::now());

    let mut file = store::read(&state.paths.vault)?
        .unwrap_or_else(|| VaultFile::new(&state.settings.device_id, None, String::new()));
    if file.kdf.is_none() {
        file.kdf = state.kdf.read().await.clone();
    }
    file.revision += 1;
    file.updated_at = Utc::now();
    let epoch = secrets::kdf_epoch(file.kdf.as_ref());
    file.vault = secrets::seal_vault(&vault, &key, &epoch).map_err(|e| e.to_string())?;
    store::write_atomic(&state.paths.vault, &file)?;

    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    state
        .pending_identities
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    Ok(sealed_payload(&vault, file.revision))
}

#[tauri::command]
pub async fn vault_save(
    state: tauri::State<'_, AppState>,
    mut vault: Vault,
) -> Result<VaultPayload, String> {
    restore_secrets(&state, &mut vault).await;
    persist_vault(&state, vault).await
}

/// One-shot probe of a host to detect its OS family for the sidebar glyph.
/// Best-effort: on failure the host keeps whatever OS was previously recorded.
#[tauri::command]
pub async fn host_probe_os(
    state: tauri::State<'_, AppState>,
    host_id: String,
) -> Result<VaultPayload, String> {
    let id: Uuid = host_id
        .parse()
        .map_err(|_| "Invalid host id".to_string())?;

    let vault = state.unlocked_vault().await?;
    let host = vault.host(&id).cloned().ok_or("Host not found")?;

    let identity = match &host.auth {
        AuthMethod::Key { identity_id, .. } => vault.identity(identity_id).cloned(),
        _ => None,
    };
    let target = SshTarget::from_host(&host, identity.as_ref());
    let os = ssh::probe_os(target).await.map_err(|e| e.to_string())?;

    // Unknown OS: report it but never persist; the UI keeps letter avatars.
    let known = os != "unknown";
    let mut vault = vault;
    if known {
        let mut changed = false;
        for host in vault.hosts.iter_mut().filter(|h| h.id == id) {
            if host.os.as_deref() != Some(os.as_str()) {
                host.os = Some(os.clone());
                changed = true;
            }
        }
        if changed {
            return persist_vault(&state, vault).await;
        }
    }
    let revision = *state.revision.read().await;
    Ok(sealed_payload(&vault, revision))
}

/// Pull → merge (LWW + tombstones) → push to the RuTerm bridge.
#[tauri::command]
pub async fn vault_sync(state: tauri::State<'_, AppState>) -> Result<VaultPayload, String> {
    let key = state.unlocked_key().await?;
    let local = state.unlocked_vault().await?;
    let token = ensure_access_token(&state).await?;
    let bridge_url = state.bridge_url().await?;

    let mut client = BridgeClient::new(&bridge_url);
    client.set_token(Some(token));

    let mut file = store::read(&state.paths.vault)?
        .unwrap_or_else(|| VaultFile::new(&state.settings.device_id, None, String::new()));
    if file.kdf.is_none() {
        file.kdf = state.kdf.read().await.clone();
    }
    let epoch = secrets::kdf_epoch(file.kdf.as_ref());
    let sealed = secrets::seal_vault(&local, &key, &epoch).map_err(|e| e.to_string())?;

    let outcome = client
        .sync_json(
            &sealed,
            file.kdf.clone(),
            &file.canary,
            &state.settings.device_id,
            4,
        )
        .await
        .map_err(|e| e.to_string())?;
    let (merged, _) = secrets::open_vault_with_context(&outcome.vault, &key, &epoch).map_err(|e| {
        format!(
            "the synced vault cannot be opened with this passphrase ({e}). \
             Every device must use the same vault passphrase."
        )
    })?;
    // R-13: validate the merged remote vault before persisting it.
    validate_vault(&merged)?;

    file.revision = outcome.revision;
    file.updated_at = Utc::now();
    // Persist the merged copy under the current AAD, even when the remote copy
    // came from a pre-hardening build.
    file.vault = secrets::seal_vault(&merged, &key, &epoch).map_err(|e| e.to_string())?;
    if secrets::canary_is_legacy(&key, &file.canary, CANARY) {
        file.canary = secrets::seal_canary(&key, CANARY).map_err(|e| e.to_string())?;
    }
    if file.kdf.is_none() {
        file.kdf = outcome.kdf;
    }
    if file.canary.is_empty() {
        file.canary = outcome.canary;
    }
    store::write_atomic(&state.paths.vault, &file)?;

    *state.vault.write().await = Some(merged.clone());
    *state.revision.write().await = outcome.revision;
    mark_synced(&state, outcome.revision).await;
    Ok(sealed_payload(&merged, outcome.revision))
}

// -------------------------------------------------------------------- ssh

/// Opens an SSH shell.
///
/// The UI supplies `session_id` so it can subscribe to `ssh://<session_id>`
/// *before* the first byte arrives — otherwise a fast banner/`motd` could be
/// emitted before the listener existed and be lost.
#[tauri::command]
pub async fn ssh_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    host_id: String,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    let vault = state.unlocked_vault().await?;
    let session_id = checked_session_id(&session_id)?;
    let id: Uuid = host_id
        .parse()
        .map_err(|_| "Invalid host id".to_string())?;
    let host = vault.host(&id).cloned().ok_or("Host not found")?;

    let identity = match &host.auth {
        AuthMethod::Key { identity_id, .. } => vault.identity(identity_id).cloned(),
        _ => None,
    };
    if matches!(host.auth, AuthMethod::Key { .. }) && identity.is_none() {
        return Err("Identity not found for this host".into());
    }

    let target = SshTarget::from_host(&host, identity.as_ref());
    open_shell_session(app, state.inner(), target, session_id, cols, rows).await
}

/// Hands an SSH target to the session machinery: registers the handle and the
/// output ring buffer, then streams `ssh://<session_id>` in a background task.
async fn open_shell_session(
    app: tauri::AppHandle,
    state: &AppState,
    target: SshTarget,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    let policy = target.host_key_policy.clone();
    let host_key = tofu_host_key(&target.username, &target.host, target.port);
    let session = ssh::open_shell(
        target,
        TermSize {
            cols: cols.clamp(20, 500),
            rows: rows.clamp(5, 300),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    // F-46: persist the pin observed on first contact so future sessions are
    // strictly pinned instead of silently re-trusting a new key.
    persist_tofu(state, &host_key, &policy, &session.fingerprint)?;

    let session_id = checked_session_id(&session_id)?;
    let handle = session.handle();
    let mut ssh_guard = state.ssh.lock().map_err(|_| "state lock".to_string())?;
    // F-34: never overwrite a live SSH handle with a colliding id.
    if ssh_guard.contains_key(&session_id) {
        return Err("session id already in use".into());
    }
    ssh_guard.insert(session_id.clone(), handle);
    drop(ssh_guard);
    if let Ok(mut output) = state.output.lock() {
        output.insert(session_id.clone(), StreamEntry::new());
    }

    let channel = format!("ssh://{session_id}");
    let channel_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let mut session = session;
        while let Some(event) = session.next_event().await {
            let terminal = matches!(
                event,
                SshEvent::Closed { .. } | SshEvent::ExitStatus { .. } | SshEvent::Error { .. }
            );
            match &event {
                SshEvent::Data { data_b64 } | SshEvent::Stderr { data_b64 } => {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data_b64) {
                        if let Ok(mut output) = app.state::<AppState>().output.lock() {
                            if let Some(entry) = output.get_mut(&channel_id) {
                                entry.push(&bytes);
                            }
                        }
                    }
                }
                _ => {}
            }
            if terminal {
                if let Ok(mut output) = app.state::<AppState>().output.lock() {
                    if let Some(entry) = output.get_mut(&channel_id) {
                        entry.closed = true;
                    }
                }
            }
            let _ = app.emit(&channel, &event);
            if terminal {
                break;
            }
        }
        let _ = app.emit(
            &channel,
            SshEvent::Closed {
                reason: "session finished".into(),
            },
        );
        // Drop the dead handle so `ssh_write` fails fast instead of queueing
        // bytes into a closed channel.
        if let Ok(mut handles) = app.state::<AppState>().ssh.lock() {
            handles.remove(&channel_id);
        }
    });

    Ok(issue_session_token(state, &session_id))
}

/// Quick-connect from the home screen: no vault host required. Auth is the
/// user's SSH agent, or a one-shot password that is never written to the vault.
#[tauri::command]
pub async fn ssh_open_adhoc(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    username: String,
    address: String,
    port: u16,
    auth: String,
    password: Option<String>,
    allow_agent: bool,
    cols: u32,
    rows: u32,
) -> Result<String, String> {
    let session_id = checked_session_id(&session_id)?;
    let username = username.trim().to_string();
    let address = address.trim().to_string();
    if username.is_empty() || address.is_empty() {
        return Err("Type ssh user@host:port".into());
    }
    if port == 0 {
        return Err("Port must be between 1 and 65535".into());
    }
    let auth = match auth.as_str() {
        // F-06: offering the SSH agent to an arbitrary host requires an
        // explicit, per-connection user confirmation.
        "agent" if allow_agent => AuthMethod::Agent,
        "agent" => return Err("Confirm SSH agent use for this host before connecting".into()),
        "password" => AuthMethod::Password {
            password: password.unwrap_or_default(),
        },
        _ => return Err("Unknown auth method".into()),
    };
    let policy = tofu_policy(state.inner(), &username, &address, port);
    let target = SshTarget {
        host: address,
        port,
        username,
        auth,
        identity: None,
        timeout: Duration::from_secs(15),
        keepalive_secs: 30,
        term: "xterm-256color".into(),
        host_key_policy: policy,
        env: Vec::new(),
    };
    open_shell_session(app, state.inner(), target, session_id, cols, rows).await
}

#[tauri::command]
pub async fn ssh_write(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
    data_b64: String,
) -> Result<(), String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Write chunk too large".into());
    }
    let handles = state.ssh.lock().map_err(|_| "state lock".to_string())?;
    let handle = handles.get(&session_id).ok_or("Session is not active")?;
    handle.write(bytes);
    Ok(())
}

#[tauri::command]
pub async fn ssh_resize(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    let handles = state.ssh.lock().map_err(|_| "state lock".to_string())?;
    if let Some(handle) = handles.get(&session_id) {
        handle.resize(cols, rows);
    }
    Ok(())
}

#[tauri::command]
pub async fn ssh_close(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
) -> Result<(), String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    let mut handles = state.ssh.lock().map_err(|_| "state lock".to_string())?;
    if let Some(handle) = handles.remove(&session_id) {
        handle.close();
    }
    if let Ok(mut output) = state.output.lock() {
        output.remove(&session_id);
    }
    if let Ok(mut tokens) = state.session_tokens.lock() {
        tokens.remove(&session_id);
    }
    Ok(())
}

/// What a webview reload needs to restore a terminal.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachPayload {
    pub found: bool,
    /// `connected` while the PTY/shell is still alive, `closed` once it finished.
    pub status: String,
    /// Base64-encoded raw output replay (escape sequences included).
    pub data_b64: String,
    /// Exact number of replay bytes; lets the frontend drop overlap with the
    /// live stream it just subscribed to.
    pub bytes: u64,
}

/// Replays an existing session after a reload instead of opening a new one.
#[tauri::command]
pub fn session_attach(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
) -> Result<Option<AttachPayload>, String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    let map = state.output.lock().map_err(|_| "state lock".to_string())?;
    let Some(entry) = map.get(&session_id) else {
        return Ok(None);
    };
    Ok(Some(AttachPayload {
        found: true,
        status: if entry.closed { "closed".into() } else { "connected".into() },
        data_b64: base64::engine::general_purpose::STANDARD.encode(&entry.buf),
        bytes: entry.buf.len() as u64,
    }))
}

/// Reads an uploaded `.pem` / OpenSSH key and returns an `Identity` for the
/// vault. The file never leaves the machine; only the sealed copy is stored.
#[tauri::command]
pub async fn ssh_import_key(
    state: tauri::State<'_, AppState>,
    label: String,
    pem: String,
    passphrase: Option<String>,
) -> Result<Identity, String> {
    state.unlocked_key().await?;
    let pass = passphrase.filter(|value| !value.trim().is_empty());
    let inspected = ssh::inspect_private_key(&pem, pass.as_deref()).map_err(|e| e.to_string())?;
    let kind = match inspected.kind.as_str() {
        "ed25519" => IdentityKind::Ed25519,
        "rsa" => IdentityKind::Rsa,
        "ecdsa" => IdentityKind::Ecdsa,
        _ => IdentityKind::Imported,
    };
    let now = Utc::now();
    let name = label.trim();
    let identity = Identity {
        id: Uuid::new_v4(),
        label: if name.is_empty() {
            "Imported key".to_string()
        } else {
            name.to_string()
        },
        private_key: inspected.private_key,
        passphrase: pass,
        kind: Some(kind),
        fingerprint: Some(inspected.fingerprint),
        created_at: now,
        updated_at: now,
        deleted: false,
    };
    state
        .pending_identities
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(identity.id, identity.clone());
    Ok(redact_identity(&identity))
}

#[tauri::command]
pub async fn local_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
    cwd: Option<String>,
) -> Result<String, String> {
    // A local shell is at least as privileged as the vault itself: never let
    // an IPC caller spawn one while the vault is locked.
    state.unlocked_key().await?;
    let session_id = checked_session_id(&session_id)?;
    crate::local::open(app, &state.local, session_id.clone(), cols, rows, expand_cwd(cwd))?;
    Ok(issue_session_token(&state, &session_id))
}

/// Expands a leading `~/` (or bare `~`) to the user's home directory. Falls
/// back to home when the given directory does not exist — the PTY spawn
/// would fail otherwise.
fn expand_cwd(cwd: Option<String>) -> Option<String> {
    let home = std::env::var("HOME").ok();
    let dir = cwd?.trim().to_string();
    if dir.is_empty() {
        return home;
    }
    let expanded = if dir == "~" {
        home.clone()?
    } else if let Some(rest) = dir.strip_prefix("~/") {
        format!("{}/{rest}", home.clone()?)
    } else {
        dir
    };
    if std::path::Path::new(&expanded).is_dir() {
        Some(expanded)
    } else {
        home
    }
}

/// AI coding-agent CLIs RuTerm can launch in a local terminal. Detection is
/// automatic: only the tools actually on the user's PATH are returned.
#[derive(Serialize)]
pub struct AiAgent {
    pub id: String,
    pub label: String,
    /// Binary name used to spawn the agent (resolved by the login shell).
    pub command: String,
    /// Absolute path the login shell resolves it to.
    pub path: String,
}

const AI_AGENTS: &[(&str, &str, &str)] = &[
    ("opencode", "OpenCode", "opencode"),
    ("freebuff", "Freebuff", "freebuff"),
    ("kilo", "Kilo Code", "kilo"),
    ("claude", "Claude", "claude"),
    ("kiro", "Kiro", "kiro-cli"),
];

/// Detects AI-agent CLIs on the user's machine with a *single* interactive
/// login shell probe — spinning up one shell per candidate costs ~15s each.
/// Interactive (-i) so `.zshrc` style PATH extensions are honoured; the
/// `cmd=` line prefixes make the answer robust against startup-file noise.
#[tauri::command]
pub async fn ai_agents_detect() -> Result<Vec<AiAgent>, String> {
    let _permit = shell_probe_semaphore()
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let script = AI_AGENTS
        .iter()
        .map(|&(_, _, cmd)| {
            format!("printf '%s=' '{cmd}'; command -v '{cmd}' 2>/dev/null || true; printf '\\n'")
        })
        .collect::<Vec<_>>()
        .join("; ");
    let shell_for_probe = shell.clone();
    let output = tauri::async_runtime::spawn_blocking(move || {
        std::process::Command::new(&shell_for_probe)
            .args(["-lic", &script])
            .output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut found = Vec::with_capacity(AI_AGENTS.len());
    for &(id, label, command) in AI_AGENTS {
        let prefix = format!("{command}=");
        let path = stdout.lines().find_map(|line| {
            let path = line.strip_prefix(&prefix)?.trim();
            path.starts_with('/').then(|| path.to_string())
        });
        if let Some(path) = path {
            found.push(AiAgent {
                id: id.into(),
                label: label.into(),
                command: command.into(),
                path,
            });
        }
    }
    Ok(found)
}

#[tauri::command]
pub async fn local_write(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
    data_b64: String,
) -> Result<(), String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Write chunk too large".into());
    }
    crate::local::write(&state.local, &session_id, &bytes)
}

#[tauri::command]
pub async fn local_resize(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    crate::local::resize(&state.local, &session_id, cols, rows)
}

#[tauri::command]
pub async fn local_close(
    state: tauri::State<'_, AppState>,
    session_id: String,
    token: String,
) -> Result<(), String> {
    let session_id = require_session_token(&state, &session_id, &token)?;
    crate::local::close(&state.local, &session_id)?;
    if let Ok(mut output) = state.output.lock() {
        output.remove(&session_id);
    }
    if let Ok(mut tokens) = state.session_tokens.lock() {
        tokens.remove(&session_id);
    }
    Ok(())
}

/// Bounds concurrent key generation so a flood of IPC calls cannot saturate
/// every worker thread / exhaust memory (F-33).
static KEYGEN_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn keygen_semaphore() -> &'static Arc<Semaphore> {
    KEYGEN_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(2)))
}

/// Bounds concurrent `$SHELL -lic` probes from `ai_agents_detect`: each one
/// runs a full interactive login shell, so an IPC flood is a cheap DoS (F-33).
static SHELL_PROBE_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn shell_probe_semaphore() -> &'static Arc<Semaphore> {
    SHELL_PROBE_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(1)))
}

/// Generates an Ed25519/ECDSA keypair for the vault.
#[tauri::command]
pub async fn ssh_keygen(
    state: tauri::State<'_, AppState>,
    kind: String,
    label: String,
) -> Result<Identity, String> {
    state.unlocked_key().await?;
    let kind = kind.to_ascii_lowercase();
    if kind != "ed25519" && kind != "ecdsa" {
        return Err("key type must be ed25519 or ecdsa".into());
    }
    if label.len() > 200 {
        return Err("label is too long".into());
    }
    let _permit = keygen_semaphore()
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    let identity_kind = match kind.as_str() {
        "rsa" => IdentityKind::Rsa,
        "ecdsa" => IdentityKind::Ecdsa,
        _ => IdentityKind::Ed25519,
    };
    // F-33: key generation is CPU-bound; keep it off the async IPC thread.
    let generated = tokio::task::spawn_blocking(move || ssh::generate_key(&kind))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let now = Utc::now();
    let identity = Identity {
        id: Uuid::new_v4(),
        label,
        private_key: generated.private_key,
        passphrase: None,
        kind: Some(identity_kind),
        fingerprint: Some(generated.fingerprint),
        created_at: now,
        updated_at: now,
        deleted: false,
    };
    state
        .pending_identities
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(identity.id, identity.clone());
    Ok(redact_identity(&identity))
}

/// Nothing here is secret; useful for the Settings screen.
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Keeps the native title bar in step with the in-app theme.
#[tauri::command]
pub async fn ui_set_theme(window: tauri::WebviewWindow, theme: String) -> Result<(), String> {
    let value = if theme == "ember-light" || theme == "light" {
        tauri::Theme::Light
    } else {
        tauri::Theme::Dark
    };
    window.set_theme(Some(value)).map_err(|e| e.to_string())
}

/// Scales the whole webview natively (Cmd +/-), so terminals stay sharp.
#[tauri::command]
pub async fn ui_set_zoom(window: tauri::WebviewWindow, scale: f64) -> Result<(), String> {
    window
        .set_zoom(scale.clamp(0.5, 2.5))
        .map_err(|e| e.to_string())
}

// -------------------------------------------------------------------- sftp
//
// One `SftpConnection` (a dedicated SSH connection with two `sftp` subsystem
// channels: high-level I/O + raw streaming reads) per SFTP tab. The UI keys
// tabs by id; commands look the connection up here.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpOpenInfo {
    /// Canonical home directory — the panel starts here.
    pub home: String,
    /// Login name used for the connection (for the panel subtitle).
    pub user: String,
    /// Address (host or adhoc target) the session is connected to.
    pub address: String,
    pub port: u16,
    /// Rust-issued capability that every subsequent SFTP command must present.
    pub token: String,
}

async fn sftp_target_for_host(
    state: &AppState,
    host_id: &str,
) -> Result<(SshTarget, String, String, u16), String> {
    let vault = state.unlocked_vault().await?;
    let id: Uuid = host_id
        .parse()
        .map_err(|_| "Invalid host id".to_string())?;
    let host = vault.host(&id).cloned().ok_or("Host not found")?;
    let identity = match &host.auth {
        AuthMethod::Key { identity_id, .. } => vault.identity(identity_id).cloned(),
        _ => None,
    };
    if matches!(host.auth, AuthMethod::Key { .. }) && identity.is_none() {
        return Err("Identity not found for this host".into());
    }
    let target = SshTarget::from_host(&host, identity.as_ref());
    Ok((target, host.username, host.address, host.port))
}

async fn open_sftp_in_state(
    state: &AppState,
    sftp_id: String,
    target: SshTarget,
    user: String,
    address: String,
    port: u16,
) -> Result<SftpOpenInfo, String> {
    // SFTP ids double as Tauri event channels — require a real UUID and reject
    // collisions so one tab cannot take over another live session (R-01/F-34).
    let id = checked_session_id(&sftp_id)?;
    let policy = target.host_key_policy.clone();
    let host_key = tofu_host_key(&user, &address, port);
    let (session, fingerprint) = sftp::open_sftp_with_fingerprint(&target)
        .await
        .map_err(|e| e.to_string())?;
    persist_tofu(state, &host_key, &policy, &fingerprint)?;
    let home = sftp::home_dir(&session).await;
    let mut map = state.sftp.lock().await;
    if map.contains_key(&id) {
        return Err("SFTP session id already in use".into());
    }
    map.insert(id.clone(), session);
    drop(map);
    let token = issue_sftp_token(state, &id);
    Ok(SftpOpenInfo {
        home,
        user,
        address,
        port,
        token,
    })
}

/// Opens an SFTP session against a saved vault host (any auth method — the
/// vault holds the credentials).
#[tauri::command]
pub async fn sftp_open_host(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    host_id: String,
) -> Result<SftpOpenInfo, String> {
    let (target, user, address, port) = sftp_target_for_host(state.inner(), &host_id).await?;
    open_sftp_in_state(state.inner(), sftp_id, target, user, address, port).await
}

/// Quick-connect SFTP: agent auth only — passwords are never kept in memory,
/// and the vault has nothing stored for adhoc targets.
#[tauri::command]
pub async fn sftp_open_adhoc(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    username: String,
    address: String,
    port: u16,
    allow_agent: bool,
) -> Result<SftpOpenInfo, String> {
    // F-06: quick-connect SFTP always offers the agent; require explicit opt-in.
    if !allow_agent {
        return Err("Confirm SSH agent use for this host before connecting".into());
    }
    let username = username.trim().to_string();
    let address = address.trim().to_string();
    if username.is_empty() || address.is_empty() {
        return Err("Type ssh user@host:port".into());
    }
    if port == 0 {
        return Err("Port must be between 1 and 65535".into());
    }
    let target = SshTarget {
        host: address.clone(),
        port,
        username: username.clone(),
        auth: AuthMethod::Agent,
        identity: None,
        timeout: Duration::from_secs(15),
        keepalive_secs: 30,
        term: "xterm-256color".into(),
        host_key_policy: tofu_policy(state.inner(), &username, &address, port),
        env: Vec::new(),
    };
    open_sftp_in_state(state.inner(), sftp_id, target, username, address, port).await
}

/// Lists one directory. Returns the canonical path it was resolved to.
#[tauri::command]
pub async fn sftp_list(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
) -> Result<sftp::SftpDir, String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::list_dir(session, &safe_remote_path(&path)?)
        .await
        .map_err(|e| e.to_string())
}

/// Creates one folder (the full path, including the new folder's name).
#[tauri::command]
pub async fn sftp_mkdir(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::create_dir(session, &safe_remote_path(&path)?)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_rename(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    from: String,
    to: String,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::rename(session, &safe_remote_path(&from)?, &safe_remote_path(&to)?)
        .await
        .map_err(|e| e.to_string())
}

/// Deletes a file, symlink, or a directory tree (the UI confirms first).
#[tauri::command]
pub async fn sftp_remove(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
    is_dir: bool,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::remove_path(session, &safe_remote_path(&path)?, is_dir)
        .await
        .map_err(|e| e.to_string())
}

/// Shows a native "choose a file to upload" dialog inside Rust and returns a
/// one-shot handle. The renderer cannot invent a local path this way (F-05).
#[tauri::command]
pub async fn sftp_pick_upload(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let picked = app
        .dialog()
        .file()
        .blocking_pick_file()
        .ok_or("No file selected")?;
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let token = Uuid::new_v4().to_string();
    state
        .local_file_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), path);
    Ok(token)
}

/// Multi-file variant used by the upload button.
#[tauri::command]
pub async fn sftp_pick_uploads(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let picked = app
        .dialog()
        .file()
        .blocking_pick_files()
        .ok_or("No files selected")?;
    let mut handles = Vec::with_capacity(picked.len());
    for file in picked {
        let path = file.into_path().map_err(|e| e.to_string())?;
        let token = Uuid::new_v4().to_string();
        state
            .local_file_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(token.clone(), path);
        handles.push(token);
    }
    Ok(handles)
}

/// Shows a native "save as" dialog inside Rust and returns a one-shot handle.
#[tauri::command]
pub async fn sftp_pick_download(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    default_name: String,
) -> Result<String, String> {
    let picked = app
        .dialog()
        .file()
        .set_file_name(default_name)
        .blocking_save_file()
        .ok_or("No destination selected")?;
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let token = Uuid::new_v4().to_string();
    state
        .local_file_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), path);
    Ok(token)
}

/// Rejects relative / dot-dot / NUL paths coming from a remote listing before
/// they reach any destructive SFTP operation (F-52).
fn safe_remote_path(path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() || !trimmed.starts_with('/') || trimmed.contains('\0') {
        return Err("Invalid remote path".into());
    }
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| *s == "." || *s == "..") {
        return Err("Remote path contains a traversal segment".into());
    }
    Ok(format!("/{}", segments.join("/")))
}

fn take_local_path(state: &AppState, token: &str) -> Result<PathBuf, String> {
    state
        .local_file_tokens
        .lock()
        .map_err(|_| "state lock".to_string())?
        .remove(token)
        .ok_or_else(|| "Invalid or expired file handle".to_string())
}

/// Uploads one local file chosen through [`sftp_pick_upload`] into
/// `remote_dir`, keeping the local file name.
#[tauri::command]
pub async fn sftp_upload(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    local_token: String,
    remote_dir: String,
) -> Result<String, String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let local = take_local_path(&state, &local_token)?;
    let name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .ok_or("The selected file has no name")?;
    let remote = if remote_dir.trim_end_matches('/').ends_with('/') || remote_dir.trim() == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", remote_dir.trim_end_matches('/'))
    };
    let remote = safe_remote_path(&remote)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::upload(session, &local, &remote)
        .await
        .map_err(|e| e.to_string())?;
    Ok(remote)
}

/// Upload from HTML5 drag & drop, where the webview only exposes the file
/// bytes (no path). Writes to `remote_path` (full path).
#[tauri::command]
pub async fn sftp_upload_bytes(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    remote_path: String,
    bytes_b64: String,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes_b64.as_bytes())
        .map_err(|e| e.to_string())?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::upload_bytes(session, &safe_remote_path(&remote_path)?, &bytes)
        .await
        .map_err(|e| e.to_string())
}

/// Downloads one remote file to a local path chosen in a native save dialog
/// (or the default `~/Downloads` suggestion).
#[tauri::command]
pub async fn sftp_download(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    remote_path: String,
    local_token: String,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let local = take_local_path(&state, &local_token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::download(session, &safe_remote_path(&remote_path)?, &local)
        .await
        .map_err(|e| e.to_string())
}

/// Reads one remote text file for the in-app viewer/editor (2 MB cap;
/// binary files come back with `content: null`).
#[tauri::command]
pub async fn sftp_read_file(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
) -> Result<sftp::SftpFilePayload, String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::read_file(session, &safe_remote_path(&path)?)
        .await
        .map_err(|e| e.to_string())
}

/// Reads one remote image for the in-app preview (12 MB cap), returned as
/// base64 so the UI can build a `data:` URL without a second round trip.
#[tauri::command]
pub async fn sftp_read_image(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
) -> Result<sftp::SftpImagePayload, String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::read_image(session, &safe_remote_path(&path)?)
        .await
        .map_err(|e| e.to_string())
}

/// Writes one remote text file back (temp file + rename, permissions
/// preserved). Never truncates the original on failure.
#[tauri::command]
pub async fn sftp_write_file(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::write_file(session, &safe_remote_path(&path)?, content.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

/// Creates a new, empty remote file. Fails if it already exists.
#[tauri::command]
pub async fn sftp_create_file(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
    path: String,
) -> Result<(), String> {
    let sftp_id = require_sftp_token(&state, &sftp_id, &token)?;
    let map = state.sftp.lock().await;
    let session = map.get(&sftp_id).ok_or("SFTP session is not open")?;
    sftp::create_file(session, &safe_remote_path(&path)?)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sftp_close(
    state: tauri::State<'_, AppState>,
    sftp_id: String,
    token: String,
) -> Result<(), String> {
    let id = require_sftp_token(&state, &sftp_id, &token)?;
    // Consume the capability so a replayed close cannot touch another session.
    state
        .sftp_tokens
        .lock()
        .map_err(|_| "state lock".to_string())?
        .remove(&id);
    let mut map = state.sftp.lock().await;
    if let Some(session) = map.remove(&id) {
        let _ = session.close().await;
    }
    Ok(())
}

#[allow(dead_code)]
fn _assert_settings_serializable(settings: &AppSettings) -> String {
    serde_json::to_string(settings).unwrap_or_default()
}
