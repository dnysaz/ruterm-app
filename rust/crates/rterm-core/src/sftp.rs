//! SFTP sessions on the same russh transport as the terminal.
//!
//! An SFTP tab opens its own channel with subsystem `sftp` after the shared
//! connect+authenticate handshake (`crate::ssh::connect_and_authenticate`).
//! One [`SftpSession`] is kept per tab by the Tauri layer; this module is
//! stateless pure-async functions over it.

use std::path::Path;

pub use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy};

use crate::ssh::{SshError, SshTarget, connect_and_authenticate};

/// One directory entry, shipped to the UI.
#[derive(Serialize, Clone, Debug)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// `None` when the server omits the size (it always sends one, but don't crash on it).
    pub size: Option<u64>,
    /// Seconds since the epoch, as reported by the server.
    pub mtime: Option<u32>,
    /// e.g. `drwxr-xr-x`. `None` when the server omits permissions.
    pub permissions: Option<String>,
}

/// A directory listing. `path` is the *canonical* (absolute) path — listings
/// always report where they were actually taken from.
#[derive(Serialize, Clone, Debug)]
pub struct SftpDir {
    pub path: String,
    pub entries: Vec<SftpEntry>,
}

fn err(context: &str, e: SftpError) -> SshError {
    SshError::Request(format!("{context}: {e}"))
}

fn mode_string(mode: Option<u32>) -> Option<String> {
    let m = mode?;
    let type_char = match m & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o010_000 => 'p',
        _ => '-',
    };
    let mut out = String::with_capacity(10);
    out.push(type_char);
    // Three triplets: owner (8..6), group (5..3), other (2..0), each r/w/x.
    for shift in [6i32, 3, 0] {
        // Special bit paired with each triplet: setuid(11) / setgid(10) / sticky(9).
        let special_bit = match shift {
            6 => 11,
            3 => 10,
            _ => 9,
        };
        let has_exec = m & (1 << shift) != 0;
        let special = m & (1 << special_bit) != 0;
        out.push(if m & (1 << (shift + 2)) != 0 { 'r' } else { '-' });
        out.push(if m & (1 << (shift + 1)) != 0 { 'w' } else { '-' });
        out.push(match (has_exec, special, shift) {
            (true, true, 0) => 't',
            (false, true, 0) => 'T',
            (true, true, _) => 's',
            (false, true, _) => 'S',
            (true, false, _) => 'x',
            (false, false, _) => '-',
        });
    }
    Some(out)
}

fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub(crate) fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None; // already at root
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(trimmed[..i].to_string()),
        None => None,
    }
}

/// Connects, authenticates, opens the `sftp` subsystem channel, and performs
/// the SFTP handshake. Returns a session the caller keeps open.
pub async fn open_sftp(target: &SshTarget) -> Result<SftpSession, SshError> {
    let auth = connect_and_authenticate(target).await?;
    let channel = auth.session.channel_open_session().await?;
    channel
        .request_subsystem(false, "sftp")
        .await
        .map_err(|e| SshError::Request(format!("sftp subsystem refused: {e}")))?;
    let stream = channel.into_stream();
    let sftp = SftpSession::new(stream)
        .await
        .map_err(|e| SshError::Request(format!("sftp init failed: {e}")))?;
    sftp.set_timeout(30);
    Ok(sftp)
}

/// Best-effort home directory (SFTP servers start in the user's home).
/// Falls back to `/` when the server refuses to canonicalize.
pub async fn home_dir(sftp: &SftpSession) -> String {
    sftp.canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_string())
}

/// Lists one directory. Always returns entries sorted directories-first, then
/// by name (case-insensitive). Canonicalizes the path first.
pub async fn list_dir(sftp: &SftpSession, path: &str) -> Result<SftpDir, SshError> {
    let canonical = sftp
        .canonicalize(path)
        .await
        .map_err(|e| err("cannot resolve path", e))?;
    let read = sftp
        .read_dir(&canonical)
        .await
        .map_err(|e| err("cannot list directory", e))?;
    let mut entries: Vec<SftpEntry> = read
        .map(|entry| {
            let md = entry.metadata();
            SftpEntry {
                name: entry.file_name(),
                path: entry.path(),
                is_dir: md.is_dir(),
                is_symlink: md.is_symlink(),
                size: md.size,
                mtime: md.mtime,
                permissions: mode_string(md.permissions),
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(SftpDir {
        path: canonical,
        entries,
    })
}

pub async fn create_dir(sftp: &SftpSession, path: &str) -> Result<(), SshError> {
    sftp.create_dir(path)
        .await
        .map_err(|e| err("cannot create folder", e))
}

pub async fn rename(sftp: &SftpSession, from: &str, to: &str) -> Result<(), SshError> {
    sftp.rename(from, to)
        .await
        .map_err(|e| err("cannot rename", e))
}

/// Removes a file, a symlink, or a directory tree. Directories are deleted
/// non-empty — the UI is explicit about it ("Delete folder and N items").
pub async fn remove_path(sftp: &SftpSession, path: &str, is_dir: bool) -> Result<(), SshError> {
    remove_recursive(sftp, path, is_dir, 0).await
}

const MAX_DELETE_DEPTH: u32 = 64;

fn remove_recursive<'a>(
    sftp: &'a SftpSession,
    path: &'a str,
    is_dir: bool,
    depth: u32,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SshError>> + Send + 'a>> {
    Box::pin(async move {
        if depth > MAX_DELETE_DEPTH {
            return Err(SshError::Request("delete too deep — refusing".into()));
        }
        if !is_dir {
            return sftp
                .remove_file(path)
                .await
                .map_err(|e| err("cannot delete file", e));
        }
        // A directory (or a symlink to one): delete children first.
        let read = sftp
            .read_dir(path)
            .await
            .map_err(|e| err("cannot read folder to delete", e))?;
        for entry in read {
            let md = entry.metadata();
            let child = join_path(path, &entry.file_name());
            if md.is_dir() && !md.is_symlink() {
                remove_recursive(sftp, &child, true, depth + 1).await?;
            } else {
                sftp.remove_file(&child)
                    .await
                    .map_err(|e| err("cannot delete entry", e))?;
            }
        }
        sftp.remove_dir(path)
            .await
            .map_err(|e| err("cannot remove folder", e))
    })
}

/// Streams one local file to the remote path. Creates or truncates.
pub async fn upload(sftp: &SftpSession, local: &Path, remote: &str) -> Result<(), SshError> {
    let mut local_file = tokio::fs::File::open(local)
        .await
        .map_err(|e| SshError::Request(format!("cannot open {}: {e}", local.display())))?;
    let mut remote_file = sftp
        .create(remote)
        .await
        .map_err(|e| err("cannot create remote file", e))?;
    copy(&mut local_file, &mut remote_file)
        .await
        .map_err(|e| SshError::Request(format!("upload failed: {e}")))?;
    remote_file
        .close()
        .await
        .map_err(|e| SshError::Request(format!("upload finish failed: {e}")))?;
    Ok(())
}

/// Writes an in-memory buffer to the remote path — used for HTML5 drag &
/// drop uploads, where the webview only exposes file bytes (no path).
pub async fn upload_bytes(sftp: &SftpSession, remote: &str, bytes: &[u8]) -> Result<(), SshError> {
    let mut remote_file = sftp
        .create(remote)
        .await
        .map_err(|e| err("cannot create remote file", e))?;
    remote_file
        .write_all(bytes)
        .await
        .map_err(|e| SshError::Request(format!("upload failed: {e}")))?;
    remote_file
        .close()
        .await
        .map_err(|e| SshError::Request(format!("upload finish failed: {e}")))?;
    Ok(())
}

/// Streams one remote file to a local path (overwriting).
pub async fn download(sftp: &SftpSession, remote: &str, local: &Path) -> Result<(), SshError> {
    let mut remote_file = sftp
        .open(remote)
        .await
        .map_err(|e| err("cannot open remote file", e))?;
    let mut local_file = tokio::fs::File::create(local)
        .await
        .map_err(|e| SshError::Request(format!("cannot create {}: {e}", local.display())))?;
    copy(&mut remote_file, &mut local_file)
        .await
        .map_err(|e| SshError::Request(format!("download failed: {e}")))?;
    local_file
        .flush()
        .await
        .map_err(|e| SshError::Request(format!("download finish failed: {e}")))?;
    Ok(())
}

/// Parent directory of `path`, or `None` at the filesystem root.
pub fn parent_of(path: &str) -> Option<String> {
    parent_path(path)
}

// ------------------------------------------------------------------ file editor

/// One full remote file, shipped to the in-app viewer/editor.
#[derive(Serialize, Clone, Debug)]
pub struct SftpFilePayload {
    /// UTF-8 decoded content; `None` when the file is binary or not UTF-8.
    pub content: Option<String>,
    /// Bytes actually read (≤ `MAX_FILE_BYTES`).
    pub size: u64,
}

/// Raw bytes of one remote image, base64-encoded for a `data:` URL in the UI.
#[derive(Serialize, Clone, Debug)]
pub struct SftpImagePayload {
    /// Base64 of the file bytes.
    pub data: String,
    /// Bytes actually read (≤ `MAX_IMAGE_BYTES`).
    pub size: u64,
}

/// 12 MiB — screenshots and phone photos fit, a runaway file does not.
pub const MAX_IMAGE_BYTES: u64 = 12 * 1024 * 1024;

/// 2 MiB. Text files above this are refused — they are not meant to be
/// edited in a light editor, and loading them would stall the UI.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Reads one remote text file with a hard size cap.
///
/// Returns the content as text; a binary payload (invalid UTF-8) yields
/// `content: None`. Oversized files are reported as `SshError::Request` with
/// a human-readable reason so the UI can offer "Download instead".
pub async fn read_file(sftp: &SftpSession, path: &str) -> Result<SftpFilePayload, SshError> {
    let meta = sftp
        .metadata(path)
        .await
        .map_err(|e| err("cannot stat file", e))?;
    if meta.is_dir() {
        return Err(SshError::Request(
            "this is a folder — open it, don't edit it".into(),
        ));
    }
    if let Some(size) = meta.size {
        if size > MAX_FILE_BYTES {
            return Err(SshError::Request(format!(
                "file is {} MB — the in-app viewer limit is 2 MB",
                size / (1024 * 1024) + u64::from(size % (1024 * 1024) > 0)
            )));
        }
    }
    let mut remote = sftp
        .open(path)
        .await
        .map_err(|e| err("cannot open file", e))?;
    let mut bytes = Vec::new();
    // `take` guards against a missing or wrong size attribute from the server.
    tokio::io::AsyncReadExt::take(&mut remote, MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| SshError::Request(format!("cannot read file: {e}")))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(SshError::Request(
            "file grew past the 2 MB in-app viewer limit".into(),
        ));
    }
    let size = bytes.len() as u64;
    let content = String::from_utf8(bytes).ok();
    Ok(SftpFilePayload { content, size })
}

/// Reads one remote image file with a larger cap than the text viewer.
///
/// The UI decides what counts as an image (by extension) and turns the base64
/// into a `data:` URL, so this function stays format-agnostic.
pub async fn read_image(sftp: &SftpSession, path: &str) -> Result<SftpImagePayload, SshError> {
    let meta = sftp
        .metadata(path)
        .await
        .map_err(|e| err("cannot stat file", e))?;
    if meta.is_dir() {
        return Err(SshError::Request(
            "this is a folder — open it, don't preview it".into(),
        ));
    }
    if let Some(size) = meta.size {
        if size > MAX_IMAGE_BYTES {
            return Err(SshError::Request(format!(
                "image is {} MB — the in-app preview limit is 12 MB",
                size / (1024 * 1024) + u64::from(size % (1024 * 1024) > 0)
            )));
        }
    }
    let mut remote = sftp
        .open(path)
        .await
        .map_err(|e| err("cannot open file", e))?;
    let mut bytes = Vec::new();
    tokio::io::AsyncReadExt::take(&mut remote, MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| SshError::Request(format!("cannot read file: {e}")))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(SshError::Request(
            "file grew past the 12 MB in-app preview limit".into(),
        ));
    }
    use base64::Engine as _;
    Ok(SftpImagePayload {
        size: bytes.len() as u64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Writes one remote text file back.
///
/// The bytes go to a unique sibling (`<path>.rterm-<rand>`) and are renamed
/// over the target, so a failed write never truncates the original. The new
/// file inherits the original permission bits when the server reports them.
pub async fn write_file(sftp: &SftpSession, path: &str, content: &[u8]) -> Result<(), SshError> {
    // Preserve permission bits of the existing file, if any.
    let permissions = sftp
        .metadata(path)
        .await
        .ok()
        .and_then(|m| m.permissions.map(|p| p & 0o7777));

    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let tmp = format!("{path}.rterm-{suffix}");

    let attrs = russh_sftp::protocol::FileAttributes {
        permissions,
        ..Default::default()
    };
    let flags = russh_sftp::protocol::OpenFlags::WRITE
        | russh_sftp::protocol::OpenFlags::CREATE
        | russh_sftp::protocol::OpenFlags::TRUNCATE;

    let write_result = async {
        let mut file = sftp
            .open_with_flags_and_attributes(&tmp, flags, attrs)
            .await
            .map_err(|e| err("cannot create temporary file on the server", e))?;
        file.write_all(content)
            .await
            .map_err(|e| SshError::Request(format!("write failed: {e}")))?;
        file.close()
            .await
            .map_err(|e| SshError::Request(format!("write finish failed: {e}")))?;
        Ok::<(), SshError>(())
    }
    .await;

    if let Err(e) = write_result {
        let _ = sftp.remove_file(&tmp).await;
        return Err(e);
    }
    let rename_err = match sftp.rename(&tmp, path).await {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };

    // Not every SFTP server lets a rename replace an existing file (Windows
    // hosts, NAS boxes and some embedded/older implementations answer with a
    // bare Failure). The bytes were already written successfully, so overwrite
    // the target in place — that keeps the edit and touches nothing else.
    let in_place = write_target(sftp, path, content, flags).await;
    if in_place.is_ok() {
        let _ = sftp.remove_file(&tmp).await;
        return Ok(());
    }

    // The target may be read-only while its folder is writable: drop it and
    // rename the complete temp file into place instead. If even that fails, the
    // bytes are still in memory, so recreate the file rather than lose it.
    if sftp.remove_file(path).await.is_ok() {
        if sftp.rename(&tmp, path).await.is_ok() {
            return Ok(());
        }
        if write_target(sftp, path, content, flags).await.is_ok() {
            let _ = sftp.remove_file(&tmp).await;
            return Ok(());
        }
    }

    let _ = sftp.remove_file(&tmp).await;
    let write_err = in_place.err().map(|e| e.to_string()).unwrap_or_default();
    Err(SshError::Request(format!(
        "could not replace the file with the new version ({rename_err}); \
         writing it directly also failed ({write_err})"
    )))
}

/// Truncating write of a whole file — used when the server refuses the
/// temp-file rename dance. Returns the same errors the caller reports.
async fn write_target(
    sftp: &SftpSession,
    path: &str,
    content: &[u8],
    flags: russh_sftp::protocol::OpenFlags,
) -> Result<(), SshError> {
    let mut file = sftp
        .open_with_flags(path, flags)
        .await
        .map_err(|e| err("cannot open the file for writing", e))?;
    file.write_all(content)
        .await
        .map_err(|e| SshError::Request(format!("write failed: {e}")))?;
    file.close()
        .await
        .map_err(|e| SshError::Request(format!("write finish failed: {e}")))?;
    Ok(())
}

/// Creates a new, empty remote file. Fails if the path already exists —
/// never silently truncates someone's file.
pub async fn create_file(sftp: &SftpSession, path: &str) -> Result<(), SshError> {
    let flags = russh_sftp::protocol::OpenFlags::WRITE
        | russh_sftp::protocol::OpenFlags::CREATE
        | russh_sftp::protocol::OpenFlags::EXCLUDE;
    let file = sftp
        .open_with_flags(path, flags)
        .await
        .map_err(|e| err("cannot create the file — it may already exist", e))?;
    file.close()
        .await
        .map_err(|e| SshError::Request(format!("create finish failed: {e}")))
}
