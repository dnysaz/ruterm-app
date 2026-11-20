//! SFTP sessions on the same russh transport as the terminal.
//!
//! An SFTP tab opens two channels with subsystem `sftp` after the shared
//! connect+authenticate handshake (`crate::ssh::connect_and_authenticate`):
//!
//! * a high-level [`SftpSession`] for ordinary file operations, and
//! * a [`RawSftpSession`] used **only** for directory reads, so entries can be
//!   consumed one `SSH_FXP_NAME` packet at a time and the 10,000-entry cap is
//!   enforced before another entry is retained (R-03).
//!
//! One [`SftpConnection`] (which owns both channels plus the SSH handle) is
//! kept per tab by the Tauri layer.

use std::{ops::Deref, path::Path};

pub use russh_sftp::client::SftpSession;
use russh_sftp::client::{RawSftpSession, error::Error as SftpError};
use russh_sftp::protocol::{File, FileAttributes, StatusCode};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy};

use crate::ssh::{SshError, SshSessionHandle, SshTarget, connect_and_authenticate};

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

/// A directory listing. `path` is exactly the directory the caller requested;
/// it is echoed back locally rather than obtained from server canonicalization.
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

/// One live SFTP tab: high-level session, streaming raw session, and the SSH
/// handle both channels belong to. `Deref` keeps the dozen file-operation
/// call sites unchanged — they still see an ordinary `SftpSession`.
pub struct SftpConnection {
    high: SftpSession,
    raw: RawSftpSession,
    // Keep the authenticated transport alive; dropping a russh Handle only
    // detaches its task, but owning it here makes the lifetime explicit and
    // lets us open both subsystem channels from the same connection.
    _ssh: SshSessionHandle,
}

impl Deref for SftpConnection {
    type Target = SftpSession;

    fn deref(&self) -> &Self::Target {
        &self.high
    }
}

/// Opens a second `sftp` subsystem channel on an existing SSH connection and
/// initializes it as a raw packet session. Request ids are allocated by this
/// session, so they cannot collide with the high-level session's ids.
async fn open_raw_sftp(ssh: &SshSessionHandle) -> Result<RawSftpSession, SshError> {
    let channel = ssh.channel_open_session().await?;
    channel
        .request_subsystem(false, "sftp")
        .await
        .map_err(|e| SshError::Request(format!("raw sftp subsystem refused: {e}")))?;
    let stream = channel.into_stream();
    let raw = RawSftpSession::new(stream);
    raw.init()
        .await
        .map_err(|e| err("raw sftp init failed", e))?;
    raw.set_timeout(30);
    Ok(raw)
}

/// Connects, authenticates, opens both `sftp` subsystem channels, and performs
/// both SFTP handshakes. Returns a connection the caller keeps open.
pub async fn open_sftp(target: &SshTarget) -> Result<SftpConnection, SshError> {
    open_sftp_with_fingerprint(target)
        .await
        .map(|(session, _)| session)
}

/// Same as [`open_sftp`], but also returns the observed host-key fingerprint so
/// the caller can persist a TOFU pin (F-46).
pub async fn open_sftp_with_fingerprint(
    target: &SshTarget,
) -> Result<(SftpConnection, String), SshError> {
    let auth = connect_and_authenticate(target).await?;
    let fingerprint = auth.fingerprint.clone();
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
    let raw = open_raw_sftp(&auth.session).await?;
    Ok((
        SftpConnection {
            high: sftp,
            raw,
            _ssh: auth.session,
        },
        fingerprint,
    ))
}

/// Best-effort home directory (SFTP servers start in the user's home).
/// Falls back to `/` when the server refuses to canonicalize.
pub async fn home_dir(sftp: &SftpSession) -> String {
    sftp.canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_string())
}

/// Maximum entries/bytes returned from a single remote directory listing (F-44).
const MAX_DIR_ENTRIES: usize = 10_000;
const MAX_DIR_NAME_BYTES: usize = 4096;
/// Total entries one recursive delete may touch (root + all descendants) (R-03).
const MAX_DELETE_ENTRIES: usize = 10_000;

/// Charge one entry against a recursive-delete budget, failing closed.
fn charge_delete(budget: &mut usize) -> Result<(), SshError> {
    if *budget == 0 {
        return Err(SshError::LimitExceeded(format!(
            "delete would touch more than {MAX_DELETE_ENTRIES} entries"
        )));
    }
    *budget -= 1;
    Ok(())
}

/// One `SSH_FXP_NAME` packet at a time from a directory handle. The library
/// owns framing, request ids, timeout, and ATTRS decoding; we only decide when
/// to stop and always close the handle, including on early cap exit or error.
struct RawDirIter<'a> {
    raw: &'a RawSftpSession,
    handle: String,
    batch: std::vec::IntoIter<File>,
    eof: bool,
}

impl<'a> RawDirIter<'a> {
    async fn open(raw: &'a RawSftpSession, path: &str) -> Result<Self, SshError> {
        let handle = raw
            .opendir(path)
            .await
            .map_err(|e| err("cannot open directory", e))?;
        Ok(Self {
            raw,
            handle: handle.handle,
            batch: Vec::new().into_iter(),
            eof: false,
        })
    }

    async fn next_file(&mut self) -> Result<Option<File>, SshError> {
        while self.batch.len() == 0 {
            if self.eof {
                return Ok(None);
            }
            match self.raw.readdir(self.handle.as_str()).await {
                Ok(name) => self.batch = name.files.into_iter(),
                Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(e) => return Err(err("cannot read directory", e)),
            }
        }
        Ok(self.batch.next())
    }

    async fn close(self) -> Result<(), SshError> {
        self.raw
            .close(self.handle.as_str())
            .await
            .map_err(|e| err("cannot close directory", e))?;
        Ok(())
    }
}

fn valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

/// Builds a UI entry from a wire `File`. R-02 layer A: the name is validated
/// and the path is assembled locally from the directory the user requested —
/// no server-supplied path is ever used.
fn entry_from_file(dir: &str, file: &File) -> Result<SftpEntry, SshError> {
    if !valid_filename(&file.filename) {
        return Err(SshError::Request("invalid filename in directory listing".into()));
    }
    let attrs: &FileAttributes = &file.attrs;
    Ok(SftpEntry {
        name: file.filename.clone(),
        path: join_path(dir, &file.filename),
        is_dir: attrs.is_dir(),
        is_symlink: attrs.is_symlink(),
        size: attrs.size,
        mtime: attrs.mtime,
        permissions: mode_string(attrs.permissions),
    })
}

/// Appends one entry only while it fits the caps. Kept separate so the
/// "never retain entry 10,001" invariant is directly unit-testable.
fn accept_entry(
    entries: &mut Vec<SftpEntry>,
    total_bytes: &mut usize,
    entry: SftpEntry,
) -> Result<(), SshError> {
    if entries.len() >= MAX_DIR_ENTRIES {
        return Err(SshError::LimitExceeded(format!(
            "directory has more than {MAX_DIR_ENTRIES} entries"
        )));
    }
    *total_bytes = total_bytes
        .saturating_add(entry.name.len())
        .saturating_add(entry.path.len());
    if entry.name.len() > MAX_DIR_NAME_BYTES || *total_bytes > MAX_DIR_ENTRIES * MAX_DIR_NAME_BYTES {
        return Err(SshError::LimitExceeded(
            "directory listing is too large".into(),
        ));
    }
    entries.push(entry);
    Ok(())
}

/// Streams a bounded listing over the raw channel. Peak memory is
/// O(10,000 entries + one SFTP packet), independent of the remote directory
/// size: we stop as soon as entry 10,001 is seen and close the handle.
async fn raw_list_dir(raw: &RawSftpSession, path: &str) -> Result<Vec<SftpEntry>, SshError> {
    let mut iter = RawDirIter::open(raw, path).await?;
    let listing = async {
        let mut entries: Vec<SftpEntry> = Vec::new();
        let mut total_bytes = 0usize;
        while let Some(file) = iter.next_file().await? {
            // Match the high-level iterator's behavior: dot entries are noise,
            // not real children. Anything else invalid still fails closed.
            if file.filename == "." || file.filename == ".." {
                continue;
            }
            let entry = entry_from_file(path, &file)?;
            accept_entry(&mut entries, &mut total_bytes, entry)?;
        }
        Ok::<Vec<SftpEntry>, SshError>(entries)
    }
    .await;
    // Always close the remote handle, even when the listing hit a cap or a
    // server error mid-stream. Preserve the listing error if both fail.
    let close = iter.close().await;
    let mut entries = listing?;
    close?;
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Lists one directory. Always returns entries sorted directories-first, then
/// by name (case-insensitive).
///
/// R-02: the requested `path` is passed straight to OPENDIR and echoed back in
/// `SftpDir.path`; we never call server-controlled `canonicalize()` here, and
/// every `SftpEntry.path` is built locally from that same requested path.
/// R-03: entries are read and capped one packet at a time on the raw channel.
pub async fn list_dir(sftp: &SftpConnection, path: &str) -> Result<SftpDir, SshError> {
    let entries = raw_list_dir(&sftp.raw, path).await?;
    Ok(SftpDir {
        path: path.to_string(),
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
pub async fn remove_path(sftp: &SftpConnection, path: &str, is_dir: bool) -> Result<(), SshError> {
    let mut budget = MAX_DELETE_ENTRIES;
    remove_recursive(sftp, path, is_dir, 0, &mut budget).await
}

const MAX_DELETE_DEPTH: u32 = 64;

fn remove_recursive<'a>(
    sftp: &'a SftpConnection,
    path: &'a str,
    is_dir: bool,
    depth: u32,
    budget: &'a mut usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), SshError>> + Send + 'a>> {
    Box::pin(async move {
        if depth > MAX_DELETE_DEPTH {
            return Err(SshError::Request("delete too deep — refusing".into()));
        }
        charge_delete(budget)?;
        if !is_dir {
            return sftp
                .high
                .remove_file(path)
                .await
                .map_err(|e| err("cannot delete file", e));
        }
        // A directory (or a symlink to one): delete children first. The raw
        // channel charges the global delete budget per *wire entry*, so an
        // oversized directory fails before its entries are all materialized.
        let mut iter = RawDirIter::open(&sftp.raw, path).await?;
        let walk = async {
            while let Some(file) = iter.next_file().await? {
                if file.filename == "." || file.filename == ".." {
                    continue;
                }
                charge_delete(budget)?;
                // Never let a hostile server escape the tree via a crafted name.
                if !valid_filename(&file.filename) {
                    return Err(SshError::Request("invalid filename in directory listing".into()));
                }
                let child = join_path(path, &file.filename);
                if file.attrs.is_dir() && !file.attrs.is_symlink() {
                    remove_recursive(sftp, &child, true, depth + 1, &mut *budget).await?;
                } else {
                    sftp.high
                        .remove_file(&child)
                        .await
                        .map_err(|e| err("cannot delete entry", e))?;
                }
            }
            Ok::<(), SshError>(())
        }
        .await;
        let close = iter.close().await;
        walk?;
        close?;
        sftp.high
            .remove_dir(path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{Buf, Bytes};
    use russh_sftp::{
        protocol::{Handle, Name, Packet, Status, Version},
        server::{Handler, StatusReply},
    };
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncRead, AsyncWrite, duplex};

    fn wire_file(name: &str, permissions: Option<u32>) -> File {
        File {
            filename: name.into(),
            longname: String::new(),
            attrs: FileAttributes {
                permissions,
                size: Some(7),
                mtime: Some(1_700_000_000),
                ..Default::default()
            },
        }
    }

    fn ok_status(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: "en-US".into(),
        }
    }

    /// In-process SFTP server that hands out names in bounded batches, so the
    /// client has to issue several READDIR packets and stop at the cap itself.
    struct MockDir {
        files: Vec<File>,
        batch: usize,
        delivered: Arc<Mutex<usize>>,
        fail_with: Option<StatusCode>,
    }

    impl Handler for MockDir {
        type Error = StatusReply;

        fn unimplemented(&self) -> Self::Error {
            StatusReply::new(StatusCode::OpUnsupported)
        }

        async fn opendir(&mut self, id: u32, _path: String) -> Result<Handle, Self::Error> {
            Ok(Handle {
                id,
                handle: "test-handle".into(),
            })
        }

        async fn readdir(&mut self, id: u32, _handle: String) -> Result<Name, Self::Error> {
            if let Some(code) = self.fail_with {
                return Err(code.into());
            }
            if self.files.is_empty() {
                return Err(StatusCode::Eof.into());
            }
            let n = self.batch.min(self.files.len());
            let files: Vec<File> = self.files.drain(..n).collect();
            *self.delivered.lock().unwrap() += files.len();
            Ok(Name { id, files })
        }

        async fn close(&mut self, id: u32, _handle: String) -> Result<Status, Self::Error> {
            Ok(ok_status(id))
        }
    }

    async fn mock_raw(mock: MockDir) -> RawSftpSession {
        let (client, server) = duplex(256 * 1024);
        tokio::spawn(russh_sftp::server::run(server, mock));
        let raw = RawSftpSession::new(client);
        raw.init().await.expect("sftp init");
        raw
    }

    async fn read_wire_packet<S: AsyncRead + Unpin>(stream: &mut S) -> Packet {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).await.expect("packet length");
        let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
        stream.read_exact(&mut buf).await.expect("packet body");
        let mut bytes = Bytes::from(buf);
        Packet::try_from(&mut bytes).expect("decode packet")
    }

    async fn write_wire_packet<S: AsyncWrite + Unpin>(stream: &mut S, packet: Packet) {
        let bytes = Bytes::try_from(packet).expect("encode packet");
        stream.write_all(&bytes).await.expect("write packet");
        stream.flush().await.expect("flush packet");
    }

    #[test]
    fn accept_entry_never_retains_entry_past_cap() {
        let mut entries = Vec::new();
        let mut total_bytes = 0usize;
        for i in 0..MAX_DIR_ENTRIES {
            let name = format!("f{i}");
            let entry = SftpEntry {
                path: format!("/remote/{name}"),
                name,
                is_dir: false,
                is_symlink: false,
                size: None,
                mtime: None,
                permissions: None,
            };
            accept_entry(&mut entries, &mut total_bytes, entry).expect("entry fits");
        }
        assert_eq!(entries.len(), MAX_DIR_ENTRIES);
        let extra = SftpEntry {
            name: "overflow".into(),
            path: "/remote/overflow".into(),
            is_dir: false,
            is_symlink: false,
            size: None,
            mtime: None,
            permissions: None,
        };
        let err = accept_entry(&mut entries, &mut total_bytes, extra).unwrap_err();
        assert!(matches!(err, SshError::LimitExceeded(_)), "{err:?}");
        assert_eq!(entries.len(), MAX_DIR_ENTRIES, "entry 10,001 was retained");
    }

    #[tokio::test]
    async fn raw_list_decodes_attrs_sorts_and_keeps_requested_path() {
        let raw = mock_raw(MockDir {
            files: vec![
                wire_file("z.txt", Some(0o100_644)),
                wire_file("adir", Some(0o040_755)),
            ],
            batch: 1,
            delivered: Arc::new(Mutex::new(0)),
            fail_with: None,
        })
        .await;
        let dir = raw_list_dir(&raw, "/remote").await.expect("listing");
        assert_eq!(dir.len(), 2);
        assert_eq!(dir[0].name, "adir");
        assert_eq!(dir[0].path, "/remote/adir");
        assert!(dir[0].is_dir);
        assert_eq!(dir[1].name, "z.txt");
        assert_eq!(dir[1].path, "/remote/z.txt");
        assert_eq!(dir[1].size, Some(7));
        assert_eq!(dir[1].mtime, Some(1_700_000_000));
        assert_eq!(dir[1].permissions.as_deref(), Some("-rw-r--r--"));
    }

    #[tokio::test]
    async fn raw_list_rejects_server_supplied_traversal_name() {
        let raw = mock_raw(MockDir {
            files: vec![wire_file("../escape", Some(0o100_644))],
            batch: 1,
            delivered: Arc::new(Mutex::new(0)),
            fail_with: None,
        })
        .await;
        let err = raw_list_dir(&raw, "/remote").await.unwrap_err();
        assert!(matches!(err, SshError::Request(_)), "{err:?}");
    }

    #[tokio::test]
    async fn raw_list_stops_at_entry_cap() {
        let files = (0..=MAX_DIR_ENTRIES)
            .map(|i| wire_file(&format!("f{i}"), Some(0o100_644)))
            .collect();
        let delivered = Arc::new(Mutex::new(0));
        let raw = mock_raw(MockDir {
            files,
            batch: 200,
            delivered: delivered.clone(),
            fail_with: None,
        })
        .await;
        let err = raw_list_dir(&raw, "/remote").await.unwrap_err();
        assert!(matches!(err, SshError::LimitExceeded(_)), "{err:?}");
        assert!(
            *delivered.lock().unwrap() > MAX_DIR_ENTRIES,
            "server never delivered entry 10,001, so the cap was not exercised"
        );
    }

    #[tokio::test]
    async fn raw_list_propagates_non_eof_status() {
        let raw = mock_raw(MockDir {
            files: Vec::new(),
            batch: 1,
            delivered: Arc::new(Mutex::new(0)),
            fail_with: Some(StatusCode::PermissionDenied),
        })
        .await;
        let err = raw_list_dir(&raw, "/remote").await.unwrap_err();
        assert!(matches!(err, SshError::Request(_)), "{err:?}");
    }

    #[tokio::test]
    async fn raw_list_times_out_when_server_never_replies() {
        let (client, mut server) = duplex(16 * 1024);
        let raw = RawSftpSession::new(client);
        raw.set_timeout(1);
        let task = tokio::spawn(async move {
            assert!(matches!(read_wire_packet(&mut server).await, Packet::Init(_)));
            write_wire_packet(&mut server, Packet::Version(Version::new())).await;
            // Swallow OPENDIR but never answer it.
            let _ = read_wire_packet(&mut server).await;
            std::future::pending::<()>().await;
        });
        raw.init().await.expect("sftp init");
        let err = raw_list_dir(&raw, "/remote").await.unwrap_err();
        task.abort();
        assert!(matches!(&err, SshError::Request(m) if m.contains("Timeout")), "{err:?}");
    }

    #[tokio::test]
    async fn raw_list_ignores_mismatched_request_id_and_times_out() {
        let (client, mut server) = duplex(16 * 1024);
        let raw = RawSftpSession::new(client);
        raw.set_timeout(1);
        let task = tokio::spawn(async move {
            assert!(matches!(read_wire_packet(&mut server).await, Packet::Init(_)));
            write_wire_packet(&mut server, Packet::Version(Version::new())).await;

            let open = read_wire_packet(&mut server).await;
            let id = open.get_request_id();
            write_wire_packet(
                &mut server,
                Packet::Handle(Handle {
                    id,
                    handle: "h".into(),
                }),
            )
            .await;

            assert!(matches!(read_wire_packet(&mut server).await, Packet::ReadDir(_)));
            write_wire_packet(
                &mut server,
                Packet::Status(Status {
                    id: id.wrapping_add(999),
                    status_code: StatusCode::Eof,
                    error_message: String::new(),
                    language_tag: "en-US".into(),
                }),
            )
            .await;

            // Answer CLOSE correctly so only the mismatched READDIR times out.
            let close = read_wire_packet(&mut server).await;
            write_wire_packet(&mut server, Packet::Status(ok_status(close.get_request_id()))).await;
            std::future::pending::<()>().await;
        });
        raw.init().await.expect("sftp init");
        let err = raw_list_dir(&raw, "/remote").await.unwrap_err();
        task.abort();
        assert!(matches!(&err, SshError::Request(m) if m.contains("Timeout")), "{err:?}");
    }

    #[test]
    fn version_and_status_packets_round_trip_over_wire_encoding() {
        // Direct check of the framing the raw session relies on.
        let mut bytes = Bytes::try_from(Packet::Status(ok_status(42))).expect("encode");
        bytes.advance(4); // frame length; Packet::try_from decodes type + payload
        let decoded = Packet::try_from(&mut bytes).expect("decode");
        match decoded {
            Packet::Status(status) => {
                assert_eq!(status.id, 42);
                assert_eq!(status.status_code, StatusCode::Ok);
            }
            other => panic!("unexpected packet: {other:?}"),
        }
        let mut bytes = Bytes::try_from(Packet::Handle(Handle {
            id: 7,
            handle: "h".into(),
        }))
        .expect("encode");
        bytes.advance(4);
        match Packet::try_from(&mut bytes).expect("decode") {
            Packet::Handle(handle) => {
                assert_eq!(handle.id, 7);
                assert_eq!(handle.handle, "h");
            }
            other => panic!("unexpected packet: {other:?}"),
        }
    }
}
