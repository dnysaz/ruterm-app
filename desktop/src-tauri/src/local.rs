//! Local terminal sessions backed by a real PTY.
//!
//! The frontend reuses the `ssh://<session_id>` event channel, so a local shell
//! behaves exactly like a remote one from xterm's point of view.

use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

pub struct LocalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

pub type LocalSessions = Mutex<HashMap<String, LocalSession>>;

#[derive(Serialize, Clone)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum LocalEvent {
    Data {
        data_b64: String,
    },
    #[allow(dead_code)]
    Banner {
        message: String,
    },
    Error {
        message: String,
    },
    ExitStatus {
        code: i32,
    },
    Closed {
        reason: String,
    },
}

/// Resolve the interactive shell and its startup flags for this platform.
///
/// Windows has no `/bin/zsh`, and `SHELL` may be inherited from Git Bash as a
/// full command line (`/bin/zsh -l`), which cannot be passed to
/// `CreateProcessW` as a single program name. Prefer an explicitly resolved
/// PowerShell executable, then `cmd.exe`.
#[cfg(windows)]
fn shell() -> (String, Vec<String>) {
    fn find_on_path(name: &str) -> Option<std::path::PathBuf> {
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    }

    for name in ["pwsh.exe", "powershell.exe"] {
        if let Some(path) = find_on_path(name) {
            return (
                path.to_string_lossy().into_owned(),
                vec!["-NoLogo".to_string()],
            );
        }
    }
    ("cmd.exe".to_string(), Vec::new())
}

#[cfg(not(windows))]
fn shell() -> (String, Vec<String>) {
    let program = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    (program, vec!["-l".to_string()])
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The only environment name a renderer may inject into a local shell.
/// Everything else is assembled in Rust from a sanitised base environment.
pub const ALLOWED_IPC_ENV: &str = "OPENCODE_CONFIG_CONTENT";

pub fn is_allowed_env_name(name: &str) -> bool {
    name == ALLOWED_IPC_ENV
}

/// Inherit a small, explicitly listed set of trusted process variables, then
/// add the terminal variables in Rust. No renderer-controlled map reaches the
/// spawn call except the validated `OPENCODE_CONFIG_CONTENT` value (M8).
fn sanitized_env(cmd: &mut CommandBuilder) {
    cmd.env_clear();
    const INHERIT: &[&str] = &[
        "HOME",
        "PATH",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "SSH_AUTH_SOCK",
        "SystemRoot",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "HOMEDRIVE",
        "HOMEPATH",
        "ComSpec",
        "PATHEXT",
        "PSModulePath",
        "windir",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_IDENTIFIER",
        "PROCESSOR_LEVEL",
        "PROCESSOR_REVISION",
    ];
    for name in INHERIT {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control) {
                cmd.env(name, value);
            }
        }
    }
    #[cfg(windows)]
    if std::env::var_os("HOME").is_none() {
        cmd.env(
            "HOME",
            crate::store::home_dir().to_string_lossy().into_owned(),
        );
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "RuTerm");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
}

pub fn open(
    app: tauri::AppHandle,
    sessions: &LocalSessions,
    session_id: String,
    cols: u32,
    rows: u32,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
) -> Result<(), String> {
    let id = session_id.trim().to_string();
    if id.is_empty() {
        return Err("empty session id".into());
    }

    let size = PtySize {
        rows: rows.clamp(5, 300) as u16,
        cols: cols.clamp(20, 500) as u16,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pair = native_pty_system()
        .openpty(size)
        .map_err(|e| format!("pty: {e}"))?;

    let (program, args) = shell();
    let mut cmd = CommandBuilder::new(program);
    for arg in args {
        cmd.arg(arg);
    }
    sanitized_env(&mut cmd);
    // Per-tab environment for AI agents (e.g. OPENCODE_CONFIG_CONTENT). The
    // variable belongs to the login shell, so any command typed into it
    // inherits it — without touching the user's global config files. The
    // allowlist is enforced again here as defence in depth.
    if let Some(vars) = env {
        for (name, value) in vars {
            if is_allowed_env_name(&name) {
                cmd.env(name, value);
            }
        }
    }
    let dir = cwd.unwrap_or_else(|| crate::store::home_dir().to_string_lossy().into_owned());
    cmd.cwd(dir);

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn: {e}"))?;
    // The slave is only needed to start the process; dropping it lets the
    // master see EOF when the shell exits.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("reader: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("writer: {e}"))?;

    let mut sessions_guard = sessions.lock().map_err(|_| "state lock".to_string())?;
    // F-34: a colliding id would orphan the previous PTY/child and let one tab
    // write into another tab's shell. Refuse instead of overwriting.
    if sessions_guard.contains_key(&id) {
        return Err("session id already in use".into());
    }
    sessions_guard.insert(
        id.clone(),
        LocalSession {
            master: pair.master,
            writer,
            child,
        },
    );
    if let Ok(mut output) = app.state::<crate::state::AppState>().output.lock() {
        output.insert(id.clone(), crate::state::StreamEntry::new());
    }

    let channel = format!("ssh://{id}");
    let channel_id = id.clone();
    let thread_id = id.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut output) = app.state::<crate::state::AppState>().output.lock() {
                        if let Some(entry) = output.get_mut(&channel_id) {
                            entry.push(&buffer[..n]);
                        }
                    }
                    let payload = LocalEvent::Data {
                        data_b64: b64(&buffer[..n]),
                    };
                    if app.emit(&channel, payload).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = app.emit(
                        &channel,
                        LocalEvent::Error {
                            message: error.to_string(),
                        },
                    );
                    break;
                }
            }
        }
        if let Ok(mut output) = app.state::<crate::state::AppState>().output.lock() {
            if let Some(entry) = output.get_mut(&channel_id) {
                entry.closed = true;
            }
        }
        let _ = app.emit(&channel, LocalEvent::ExitStatus { code: 0 });
        let _ = app.emit(
            &channel,
            LocalEvent::Closed {
                reason: "shell exited".into(),
            },
        );
        if let Some(state) = app.try_state::<crate::state::AppState>() {
            if let Ok(mut map) = state.local.lock() {
                map.remove(&thread_id);
            }
        }
    });

    Ok(())
}

pub fn write(sessions: &LocalSessions, session_id: &str, bytes: &[u8]) -> Result<(), String> {
    let mut map = sessions.lock().map_err(|_| "state lock".to_string())?;
    let session = map.get_mut(session_id).ok_or("Session is not active")?;
    session.writer.write_all(bytes).map_err(|e| e.to_string())?;
    session.writer.flush().map_err(|e| e.to_string())
}

pub fn resize(
    sessions: &LocalSessions,
    session_id: &str,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let map = sessions.lock().map_err(|_| "state lock".to_string())?;
    let session = map.get(session_id).ok_or("Session is not active")?;
    session
        .master
        .resize(PtySize {
            rows: rows.clamp(5, 300) as u16,
            cols: cols.clamp(20, 500) as u16,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

pub fn close(sessions: &LocalSessions, session_id: &str) -> Result<(), String> {
    let mut map = sessions.lock().map_err(|_| "state lock".to_string())?;
    if let Some(mut session) = map.remove(session_id) {
        let _ = session.child.kill();
    }
    Ok(())
}

/// Kills every local shell (used when the vault locks). The PTY reader threads
/// see EOF/errors and their output pumps stop; already-dead sessions are simply
/// absent and ignored.
pub fn close_all(sessions: &LocalSessions) {
    if let Ok(mut map) = sessions.lock() {
        for (_, mut session) in map.drain() {
            let _ = session.child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_env_allowlist_is_strict() {
        assert!(is_allowed_env_name(ALLOWED_IPC_ENV));
        for name in [
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "PATH",
            "HOME",
            "SHELL",
            "BASH_ENV",
        ] {
            assert!(!is_allowed_env_name(name), "{name} must be rejected");
        }
        assert!(!is_allowed_env_name("OPENCODE_CONFIG_CONTENT "));
    }

    /// The PTY plumbing is the only part that cannot be checked from the UI
    /// tests, so assert a real shell writes through it.
    #[test]
    fn pty_roundtrip() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg("printf hello-pty");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let mut output = Vec::new();
        let mut buffer = [0u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => output.extend_from_slice(&buffer[..n]),
                Err(_) => break,
            }
        }
        let _ = child.wait();

        let text = String::from_utf8_lossy(&output);
        assert!(text.contains("hello-pty"), "got {text:?}");
    }
}
