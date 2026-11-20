//! rterm-core: transport-agnostic engine for the rterm SSH client.
//!
//! * [`model`]  - vault entities (hosts, keys, snippets, ...)
//! * [`crypto`] - end-to-end encryption of the vault
//! * [`ssh`]    - SSH sessions on top of russh
//! * [`sftp`]   - SFTP file transfer sessions on the same transport
//! * [`sync`]   - encrypted multi-device sync client (self-hosted server)
//! * [`bridge`] - credential-encrypted sync client for the RuTerm bridge (Worker + D1);
//!   secrets are sealed client-side, but host metadata is plain JSON on the server

pub mod bridge;
pub mod crypto;
pub mod drive;
pub mod model;
pub mod oauth;
pub mod sftp;
pub mod ssh;
pub mod sync;

pub use bridge::{BridgeClient, BridgeSession, BridgeSyncOutcome, JsonSyncDoc};
pub use crypto::{KdfParams, SecretKey, SealedVault};
pub use drive::{DriveClient, DriveDoc, DriveSyncOutcome, JsonDriveDoc, JsonSyncOutcome};
pub use model::{AuthMethod, Host, Identity, Vault};
pub use sftp::{SftpDir, SftpEntry, SftpFilePayload, SftpImagePayload};
pub use ssh::{HostKeyPolicy, ImportedKey, SshEvent, SshHandle, SshSession, SshTarget, TermSize, probe_os};
pub use sync::{DeviceInfo, SyncClient, SyncOutcome};

/// Pinned protocol version, echoed by `rterm-ffi` so the UI can detect a
/// stale native library after an app update.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
