//! rterm desktop — Tauri shell around the Rust SSH/crypto core.
//!
//! The frontend (Svelte) talks to these commands; SSH output is pushed as
//! `ssh://<session_id>` events instead of polling.

mod commands;
mod local;
mod secrets;
mod state;
mod store;

use tauri::plugin::Builder as PluginBuilder;
use tauri::{Manager, Url};

/// Only the app's own origins may be loaded in the main webview. Capabilities
/// are keyed by window label, not by document, so a successful navigation to
/// an attacker origin would otherwise expose every IPC command to it.
fn allowed_navigation(url: &Url) -> bool {
    match url.scheme() {
        "tauri" | "asset" | "ipc" => true,
        "http" | "https" => {
            let host = url.host_str().unwrap_or("");
            if matches!(host, "tauri.localhost" | "ipc.localhost" | "asset.localhost") {
                return true;
            }
            // Vite dev server, development builds only.
            cfg!(debug_assertions) && host == "127.0.0.1" && url.port() == Some(1420)
        }
        _ => false,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            PluginBuilder::<tauri::Wry, ()>::new("rterm-navigation-guard")
                .on_navigation(|_webview, url| allowed_navigation(url))
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            store::restrict_dir(&dir)?;

            let settings_path = dir.join("settings.json");
            let settings = store::read_settings(&settings_path);
            if !settings_path.exists() {
                let _ = store::write_settings(&settings_path, &settings);
            }

            let paths = state::Paths {
                vault: dir.join("vault.json"),
                settings: settings_path,
                tofu: dir.join("tofu.json"),
            };
            app.manage(state::AppState::new(paths, settings));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::open_external_link,
            commands::app_version,
            commands::ui_set_zoom,
            commands::ui_set_theme,
            commands::auth_sign_in,
            commands::auth_restore,
            commands::auth_sign_out,
            commands::vault_create,
            commands::vault_reset,
            commands::vault_unlock,
            commands::vault_adopt_remote,
            commands::vault_try_auto_unlock,
            commands::vault_saved_key_exists,
            commands::vault_remember_key,
            commands::vault_lock,
            commands::vault_get,
            commands::vault_save,
            commands::vault_sync,
            commands::json_read,
            commands::json_write,
            commands::json_download,
            commands::ssh_open,
            commands::ssh_open_adhoc,
            commands::ssh_write,
            commands::ssh_resize,
            commands::ssh_close,
            commands::ssh_keygen,
            commands::ssh_import_key,
            commands::host_probe_os,
            commands::session_attach,
            commands::local_open,
            commands::local_write,
            commands::local_resize,
            commands::local_close,
            commands::ai_agents_detect,
            commands::sftp_open_host,
            commands::sftp_open_adhoc,
            commands::sftp_list,
            commands::sftp_mkdir,
            commands::sftp_rename,
            commands::sftp_remove,
            commands::sftp_pick_upload,
            commands::sftp_pick_uploads,
            commands::sftp_upload,
            commands::sftp_upload_bytes,
            commands::sftp_pick_download,
            commands::sftp_download,
            commands::sftp_close,
            commands::sftp_read_file,
            commands::sftp_read_image,
            commands::sftp_write_file,
            commands::sftp_create_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running rterm");
}
