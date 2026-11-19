//! rterm desktop — Tauri shell around the Rust SSH/crypto core.
//!
//! The frontend (Svelte) talks to these commands; SSH output is pushed as
//! `ssh://<session_id>` events instead of polling.

mod commands;
mod local;
mod secrets;
mod state;
mod store;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;

            let settings_path = dir.join("settings.json");
            let settings = store::read_settings(&settings_path);
            if !settings_path.exists() {
                let _ = store::write_settings(&settings_path, &settings);
            }

            let paths = state::Paths {
                vault: dir.join("vault.json"),
                settings: settings_path,
            };
            app.manage(state::AppState::new(paths, settings));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::app_version,
            commands::ui_set_zoom,
            commands::ui_set_theme,
            commands::auth_sign_in,
            commands::auth_restore,
            commands::auth_sign_out,
            commands::vault_create,
            commands::vault_unlock,
            commands::vault_adopt_remote,
            commands::vault_try_auto_unlock,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running rterm");
}
