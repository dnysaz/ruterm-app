//! Tauri commands: Google sign-in, vault lifecycle, bridge sync, SSH sessions.

use base64::Engine;
use chrono::Utc;
use rterm_core::bridge::{BridgeClient, DeviceInfo};
use rterm_core::crypto::{derive_key, KdfParams};
use rterm_core::model::{AuthMethod, Identity, IdentityKind, Vault};
use rterm_core::oauth::{self, GoogleAccount};
use rterm_core::ssh::{self, SshEvent, SshTarget, TermSize};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use tauri::{Emitter, Manager};
use uuid::Uuid;

use crate::secrets;
use crate::state::{
    keyring_delete, keyring_get, keyring_set, AppState, Session, StreamEntry, KEY_OAUTH, KEY_SYNC,
    KEY_VAULT,
};
use crate::store::{self, AppSettings, VaultFile};

const CANARY: &str = "rterm-canary-v1";
const SESSION_TTL_SKEW_SECS: i64 = 60;

/// Marks the cached key as usable so launch can offer one-click unlock.
async fn remember_key(state: &AppState) {
    let mut settings = state.settings_rw.write().await;
    if !settings.key_saved {
        settings.key_saved = true;
        let _ = store::write_settings(&state.paths.settings, &settings);
    }
}

fn now_secs() -> i64 {
    Utc::now().timestamp()
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
}

#[derive(Serialize)]
pub struct VaultPayload {
    pub vault: Vault,
    pub revision: i64,
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
        sub: String::new(), // bridge users are keyed by Google sub server-side; not needed in UI
        email: session.user.email.clone(),
        name: Some(session.user.name).filter(|n| !n.is_empty()),
        picture: None,
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
    let account: GoogleAccount = keyring_get("account")
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| GoogleAccount {
            sub: String::new(),
            email: session.user.email.clone(),
            name: Some(session.user.name).filter(|n| !n.is_empty()),
            picture: None,
        });
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
            let _ = BridgeClient::new(&url).logout(&session.refresh_token).await;
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
    let canary = secrets::seal_str(&key, CANARY).map_err(|e| e.to_string())?;

    let vault = Vault::default();
    let mut file = VaultFile::new(&state.settings.device_id, Some(kdf.clone()), canary);
    file.vault = secrets::seal_vault(&vault, &key).map_err(|e| e.to_string())?;
    store::write_atomic(&state.paths.vault, &file)?;

    keyring_set(KEY_VAULT, &key.to_base64());
    remember_key(&state).await;
    *state.key.write().await = Some(key);
    *state.kdf.write().await = Some(kdf);
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(VaultPayload {
        vault,
        revision: file.revision,
    })
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
    let canary = secrets::open_str(&key, &doc.canary).map_err(|_| "Wrong passphrase".to_string())?;
    if canary != CANARY {
        return Err("Wrong passphrase".into());
    }
    let vault = secrets::open_vault(&doc.vault, &key).map_err(|e| e.to_string())?;

    let mut file = VaultFile::new(&state.settings.device_id, Some(kdf.clone()), doc.canary);
    file.revision = revision;
    file.updated_at = Utc::now();
    file.vault = doc.vault;
    store::write_atomic(&state.paths.vault, &file)?;

    keyring_set(KEY_VAULT, &key.to_base64());
    remember_key(&state).await;
    *state.key.write().await = Some(key);
    *state.kdf.write().await = Some(kdf);
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(VaultPayload {
        vault,
        revision: file.revision,
    })
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
    let Ok(key) = rterm_core::crypto::SecretKey::from_base64(&raw_key) else {
        keyring_delete(KEY_VAULT);
        return Ok(None);
    };
    match unlock_with_key(&state, &file, key).await {
        Ok(payload) => Ok(Some(payload)),
        Err(_) => {
            keyring_delete(KEY_VAULT);
            Ok(None)
        }
    }
}

async fn unlock_with_key(
    state: &AppState,
    file: &VaultFile,
    key: rterm_core::crypto::SecretKey,
) -> Result<VaultPayload, String> {
    let canary = secrets::open_str(&key, &file.canary).map_err(|_| "Wrong passphrase".to_string())?;
    if canary != CANARY {
        return Err("Wrong passphrase".into());
    }
    let vault = secrets::open_vault(&file.vault, &key).map_err(|e| e.to_string())?;

    keyring_set(KEY_VAULT, &key.to_base64());
    remember_key(&state).await;
    *state.key.write().await = Some(key);
    *state.kdf.write().await = file.kdf.clone();
    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(VaultPayload {
        vault,
        revision: file.revision,
    })
}

#[tauri::command]
pub async fn vault_lock(state: tauri::State<'_, AppState>) -> Result<(), String> {
    *state.key.write().await = None;
    *state.kdf.write().await = None;
    *state.vault.write().await = None;
    Ok(())
}

#[tauri::command]
pub async fn vault_get(state: tauri::State<'_, AppState>) -> Result<Option<VaultPayload>, String> {
    let Some(vault) = state.vault.read().await.clone() else {
        return Ok(None);
    };
    Ok(Some(VaultPayload {
        vault,
        revision: *state.revision.read().await,
    }))
}

/// Seals `vault` into the on-disk file and keeps in-memory state in sync.
async fn persist_vault(state: &AppState, mut vault: Vault) -> Result<VaultPayload, String> {
    let key = state.unlocked_key().await?;
    vault.updated_at = Some(Utc::now());

    let mut file = store::read(&state.paths.vault)?
        .unwrap_or_else(|| VaultFile::new(&state.settings.device_id, None, String::new()));
    if file.kdf.is_none() {
        file.kdf = state.kdf.read().await.clone();
    }
    file.revision += 1;
    file.updated_at = Utc::now();
    file.vault = secrets::seal_vault(&vault, &key).map_err(|e| e.to_string())?;
    store::write_atomic(&state.paths.vault, &file)?;

    *state.vault.write().await = Some(vault.clone());
    *state.revision.write().await = file.revision;
    Ok(VaultPayload {
        vault,
        revision: file.revision,
    })
}

#[tauri::command]
pub async fn vault_save(
    state: tauri::State<'_, AppState>,
    vault: Vault,
) -> Result<VaultPayload, String> {
    persist_vault(&state, vault).await
}

// --------------------------------------------------------------- raw JSON

/// A JSON file as shown in the in-app editor. `vault.json` holds sealed
/// secrets, so editing it is a structural edit, not a way to read passwords.
#[derive(Serialize)]
pub struct JsonDoc {
    pub kind: String,
    pub path: String,
    pub text: String,
}

fn json_path(state: &AppState, kind: &str) -> Result<PathBuf, String> {
    match kind {
        "vault" => Ok(state.paths.vault.clone()),
        "settings" => Ok(state.paths.settings.clone()),
        other => Err(format!("Unknown JSON file: {other}")),
    }
}

#[tauri::command]
pub async fn json_read(
    state: tauri::State<'_, AppState>,
    kind: String,
) -> Result<JsonDoc, String> {
    let path = json_path(&state, &kind)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // A missing settings.json is fine: show what the app would write.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && kind == "settings" => {
            let settings = state.settings_rw.read().await.clone();
            serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("No vault.json on this device yet — create a vault first".into())
        }
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    Ok(JsonDoc {
        kind,
        path: path.display().to_string(),
        text,
    })
}

/// Validates raw JSON before it touches disk, then keeps memory in sync.
///
/// `vault.json` must deserialize as the same `VaultFile` the app reads at
/// startup; while unlocked it must also still decrypt with the current key,
/// so a bad edit can never brick the vault or lock the user out.
#[tauri::command]
pub async fn json_write(
    state: tauri::State<'_, AppState>,
    kind: String,
    text: String,
) -> Result<Option<VaultPayload>, String> {
    match kind.as_str() {
        "vault" => {
            let file: VaultFile = serde_json::from_str(&text)
                .map_err(|e| format!("vault.json is not a valid vault: {e}"))?;
            if file.schema != 1 {
                return Err(format!(
                    "Unsupported vault schema {} — this build reads schema 1",
                    file.schema
                ));
            }

            let key = state.key.read().await.clone();
            let Some(key) = key else {
                // Locked: structural validation is all that is possible.
                store::write_atomic(&state.paths.vault, &file)?;
                *state.revision.write().await = file.revision;
                return Ok(None);
            };

            if file.kdf.is_none() {
                return Err("vault.json must keep its kdf block, or the vault can no longer be unlocked".into());
            }
            let canary = secrets::open_str(&key, &file.canary)
                .map_err(|_| "vault.json was sealed with a different key (canary failed)".to_string())?;
            if canary != CANARY {
                return Err("vault.json canary does not match this vault".into());
            }
            let vault = secrets::open_vault(&file.vault, &key).map_err(|_| {
                "A sealed secret in vault.json no longer decrypts with this vault's key".to_string()
            })?;

            store::write_atomic(&state.paths.vault, &file)?;
            *state.vault.write().await = Some(vault.clone());
            *state.kdf.write().await = file.kdf.clone();
            *state.revision.write().await = file.revision;
            Ok(Some(VaultPayload {
                vault,
                revision: file.revision,
            }))
        }
        "settings" => {
            let settings: AppSettings = serde_json::from_str(&text)
                .map_err(|e| format!("settings.json is not valid: {e}"))?;
            store::write_settings(&state.paths.settings, &settings)?;
            *state.settings_rw.write().await = settings;
            Ok(None)
        }
        other => Err(format!("Unknown JSON file: {other}")),
    }
}

/// Copies the current file to the user's Downloads folder (never overwrites)
/// and returns the path so the UI can point at it.
#[tauri::command]
pub async fn json_download(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    kind: String,
) -> Result<String, String> {
    let path = json_path(&state, &kind)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && kind == "settings" => {
            let settings = state.settings_rw.read().await.clone();
            serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("No vault.json on this device yet — create a vault first".into())
        }
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };

    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("No download folder available: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;

    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let dest = dir.join(format!("ruterm-{kind}-{stamp}.json"));
    std::fs::write(&dest, text).map_err(|e| format!("Failed to save {}: {e}", dest.display()))?;
    Ok(dest.display().to_string())
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
    Ok(VaultPayload { vault, revision })
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
    let sealed = secrets::seal_vault(&local, &key).map_err(|e| e.to_string())?;

    let mut file = store::read(&state.paths.vault)?
        .unwrap_or_else(|| VaultFile::new(&state.settings.device_id, None, String::new()));
    if file.kdf.is_none() {
        file.kdf = state.kdf.read().await.clone();
    }

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
    let merged = secrets::open_vault(&outcome.vault, &key).map_err(|e| {
        format!(
            "the synced vault cannot be opened with this passphrase ({e}). \
             Every device must use the same vault passphrase."
        )
    })?;

    file.revision = outcome.revision;
    file.updated_at = Utc::now();
    file.vault = outcome.vault;
    if file.kdf.is_none() {
        file.kdf = outcome.kdf;
    }
    if file.canary.is_empty() {
        file.canary = outcome.canary;
    }
    store::write_atomic(&state.paths.vault, &file)?;

    *state.vault.write().await = Some(merged.clone());
    *state.revision.write().await = outcome.revision;
    Ok(VaultPayload {
        vault: merged,
        revision: outcome.revision,
    })
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
) -> Result<(), String> {
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
    let session = ssh::open_shell(
        target,
        TermSize {
            cols: cols.clamp(20, 500),
            rows: rows.clamp(5, 300),
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    let session_id = session_id.trim().to_string();
    if session_id.is_empty() {
        return Err("empty session id".into());
    }
    let handle = session.handle();
    state
        .ssh
        .lock()
        .map_err(|_| "state lock".to_string())?
        .insert(session_id.clone(), handle);
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

    Ok(())
}

#[tauri::command]
pub async fn ssh_write(
    state: tauri::State<'_, AppState>,
    session_id: String,
    data_b64: String,
) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .map_err(|e| e.to_string())?;
    let handles = state.ssh.lock().map_err(|_| "state lock".to_string())?;
    let handle = handles.get(&session_id).ok_or("Session is not active")?;
    handle.write(bytes);
    Ok(())
}

#[tauri::command]
pub async fn ssh_resize(
    state: tauri::State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
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
) -> Result<(), String> {
    let mut handles = state.ssh.lock().map_err(|_| "state lock".to_string())?;
    if let Some(handle) = handles.remove(&session_id) {
        handle.close();
    }
    if let Ok(mut output) = state.output.lock() {
        output.remove(&session_id);
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
) -> Result<Option<AttachPayload>, String> {
    let map = state.output.lock().map_err(|_| "state lock".to_string())?;
    let Some(entry) = map.get(session_id.trim()) else {
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
    label: String,
    pem: String,
    passphrase: Option<String>,
) -> Result<Identity, String> {
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
    Ok(Identity {
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
    })
}

#[tauri::command]
pub async fn local_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
    cwd: Option<String>,
) -> Result<(), String> {
    crate::local::open(app, &state.local, session_id, cols, rows, cwd)
}

#[tauri::command]
pub async fn local_write(
    state: tauri::State<'_, AppState>,
    session_id: String,
    data_b64: String,
) -> Result<(), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .map_err(|e| e.to_string())?;
    crate::local::write(&state.local, &session_id, &bytes)
}

#[tauri::command]
pub async fn local_resize(
    state: tauri::State<'_, AppState>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    crate::local::resize(&state.local, &session_id, cols, rows)
}

#[tauri::command]
pub async fn local_close(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    crate::local::close(&state.local, &session_id)?;
    if let Ok(mut output) = state.output.lock() {
        output.remove(&session_id);
    }
    Ok(())
}

/// Generates an Ed25519/RSA/ECDSA keypair for the vault.
#[tauri::command]
pub fn ssh_keygen(kind: String, label: String) -> Result<Identity, String> {
    let kind = kind.to_ascii_lowercase();
    if kind != "ed25519" && kind != "ecdsa" {
        return Err("key type must be ed25519 or ecdsa".into());
    }
    let generated = ssh::generate_key(&kind).map_err(|e| e.to_string())?;
    let now = Utc::now();
    let identity_kind = match kind.as_str() {
        "rsa" => IdentityKind::Rsa,
        "ecdsa" => IdentityKind::Ecdsa,
        _ => IdentityKind::Ed25519,
    };
    Ok(Identity {
        id: Uuid::new_v4(),
        label,
        private_key: generated.private_key,
        passphrase: None,
        kind: Some(identity_kind),
        fingerprint: Some(generated.fingerprint),
        created_at: now,
        updated_at: now,
        deleted: false,
    })
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

#[allow(dead_code)]
fn _assert_settings_serializable(settings: &AppSettings) -> String {
    serde_json::to_string(settings).unwrap_or_default()
}
