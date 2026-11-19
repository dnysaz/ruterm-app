//! SSH engine built on `russh`.
//!
//! One [`SshSession`] is an actor: a task owns the russh channel, reads remote
//! output and forwards it as [`SshEvent`]s, while write/resize/close commands
//! are sent through an mpsc queue. That keeps a single owner of the channel and
//! lets the UI pump events without any locking.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use russh::keys::{
    decode_secret_key, ssh_key::HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate,
};
use russh::{client, ChannelMsg, Disconnect};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::model::{AuthMethod, Host, Identity};

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("ssh error: {0}")]
    Russh(#[from] russh::Error),
    #[error("connection timed out")]
    Timeout,
    #[error("authentication failed for user {0}")]
    AuthFailed(String),
    #[error("host key mismatch: expected {expected}, got {actual}")]
    HostKeyMismatch { expected: String, actual: String },
    #[error("host uses key auth but no identity is attached")]
    MissingIdentity,
    #[error("invalid private key: {0}")]
    InvalidKey(String),
    #[error("channel closed")]
    Closed,
    #[error("session task stopped")]
    Stopped,
    #[error("channel request failed: {0}")]
    Request(String),
    #[error("ssh agent error: {0}")]
    Agent(String),
    #[error("unsupported key kind: {0}")]
    UnsupportedKey(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u32,
    pub rows: u32,
}

impl Default for TermSize {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GeneratedKey {
    pub private_key: String,
    pub public_key: String,
    pub fingerprint: String,
}

/// rand_core 0.10 adapter over the OS entropy source.
struct SystemRng;

impl russh::keys::ssh_key::rand_core::TryRng for SystemRng {
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut buf = [0u8; 4];
        self.try_fill_bytes(&mut buf)?;
        Ok(u32::from_ne_bytes(buf))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut buf = [0u8; 8];
        self.try_fill_bytes(&mut buf)?;
        Ok(u64::from_ne_bytes(buf))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        getrandom::getrandom(dst).expect("operating system RNG failed");
        Ok(())
    }
}

impl russh::keys::ssh_key::rand_core::TryCryptoRng for SystemRng {}

/// Generates an in-memory SSH key pair (private key is OpenSSH format).
pub fn generate_key(kind: &str) -> Result<GeneratedKey, SshError> {
    use russh::keys::ssh_key::{Algorithm, EcdsaCurve, HashAlg, LineEnding};
    let algorithm = match kind {
        "ed25519" => Algorithm::Ed25519,
        "ecdsa" => Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        },
        other => return Err(SshError::UnsupportedKey(other.to_string())),
    };
    let key = russh::keys::PrivateKey::random(&mut SystemRng, algorithm)
        .map_err(|e| SshError::InvalidKey(e.to_string()))?;
    let private_key = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| SshError::InvalidKey(e.to_string()))?
        .to_string();
    let public_key = key
        .public_key()
        .to_openssh()
        .map_err(|e| SshError::InvalidKey(e.to_string()))?;
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    Ok(GeneratedKey {
        private_key,
        public_key,
        fingerprint,
    })
}

/// Result of reading an uploaded private key (`.pem`, OpenSSH, RSA/ECDSA/Ed25519).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportedKey {
    pub private_key: String,
    pub fingerprint: String,
    /// `ed25519`, `rsa`, `ecdsa` or `imported`.
    pub kind: String,
}

/// Validates an uploaded key and extracts what the vault needs to store.
///
/// Runs before the key is saved so a bad or encrypted-without-passphrase file is
/// rejected in the form instead of failing later at connect time.
pub fn inspect_private_key(pem: &str, passphrase: Option<&str>) -> Result<ImportedKey, SshError> {
    let trimmed = pem.trim();
    if trimmed.is_empty() {
        return Err(SshError::InvalidKey("the file is empty".into()));
    }
    let key = decode_secret_key(trimmed, passphrase)
        .map_err(|e| SshError::InvalidKey(e.to_string()))?;
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    let kind = match key.algorithm() {
        russh::keys::ssh_key::Algorithm::Ed25519 => "ed25519",
        russh::keys::ssh_key::Algorithm::Rsa { .. } => "rsa",
        russh::keys::ssh_key::Algorithm::Ecdsa { .. } => "ecdsa",
        _ => "imported",
    }
    .to_string();
    Ok(ImportedKey {
        private_key: trimmed.to_string(),
        fingerprint,
        kind,
    })
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum HostKeyPolicy {
    /// Accept anything (used for the very first connection preview only).
    AcceptAll,
    /// Trust on first use: accept and let the UI pin the fingerprint.
    TrustOnFirstUse,
    /// Strict: only accept the pinned fingerprint.
    Pinned { fingerprint: String },
}

impl Default for HostKeyPolicy {
    fn default() -> Self {
        HostKeyPolicy::TrustOnFirstUse
    }
}

/// Everything needed to open a connection, resolved from the vault.
#[derive(Clone, Debug)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    pub identity: Option<Identity>,
    pub timeout: Duration,
    pub keepalive_secs: u64,
    pub term: String,
    pub host_key_policy: HostKeyPolicy,
    pub env: Vec<(String, String)>,
}

impl SshTarget {
    pub fn from_host(host: &Host, identity: Option<&Identity>) -> Self {
        let policy = match &host.host_key_fingerprint {
            Some(fp) if !fp.is_empty() => HostKeyPolicy::Pinned {
                fingerprint: fp.clone(),
            },
            _ => HostKeyPolicy::TrustOnFirstUse,
        };
        Self {
            host: host.address.clone(),
            port: host.port,
            username: host.username.clone(),
            auth: host.auth.clone(),
            identity: identity.cloned(),
            timeout: Duration::from_secs(host.connect_timeout_secs.unwrap_or(15)),
            keepalive_secs: host.keepalive_secs.unwrap_or(30),
            term: host
                .terminal
                .term
                .clone()
                .unwrap_or_else(|| "xterm-256color".into()),
            host_key_policy: policy,
            env: host.env.clone(),
        }
    }
}

/// Events pushed to the UI. Terminal payloads are base64 so invalid/partial
/// UTF-8 sequences survive the JSON trip.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SshEvent {
    Connected { fingerprint: String },
    Banner { message: String },
    Data { data_b64: String },
    Stderr { data_b64: String },
    ExitStatus { code: u32 },
    Closed { reason: String },
    Error { message: String },
}

#[derive(Debug, Default)]
struct Observed {
    fingerprint: Option<String>,
    banner: Option<String>,
}

pub(crate) struct Authed {
    pub session: client::Handle<Handler>,
    pub fingerprint: String,
    pub banner: Option<String>,
}

/// Connects, authenticates, and enforces the pinned host key policy.
/// Shared by the terminal (`open_shell`) and SFTP sessions.
pub(crate) async fn connect_and_authenticate(target: &SshTarget) -> Result<Authed, SshError> {
    let fingerprint_slot = Arc::new(Mutex::new(Observed::default()));
    let handler = Handler {
        policy: target.host_key_policy.clone(),
        observed: fingerprint_slot.clone(),
    };

    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(target.keepalive_secs.max(5))),
        keepalive_max: 3,
        nodelay: true,
        ..Default::default()
    });

    let addr = (target.host.as_str(), target.port);
    let mut session = tokio::time::timeout(target.timeout, client::connect(config, addr, handler))
        .await
        .map_err(|_| SshError::Timeout)??;

    authenticate(&mut session, target).await?;

    let (fingerprint, banner) = {
        let observed = fingerprint_slot.lock().expect("observed lock");
        (
            observed.fingerprint.clone().unwrap_or_default(),
            observed.banner.clone(),
        )
    };

    if let HostKeyPolicy::Pinned { fingerprint: expected } = &target.host_key_policy {
        if expected != &fingerprint && !fingerprint.is_empty() {
            return Err(SshError::HostKeyMismatch {
                expected: expected.clone(),
                actual: fingerprint,
            });
        }
    }

    Ok(Authed {
        session,
        fingerprint,
        banner,
    })
}

pub(crate) struct Handler {
    policy: HostKeyPolicy,
    observed: Arc<Mutex<Observed>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        if let Ok(mut observed) = self.observed.lock() {
            observed.fingerprint = Some(fingerprint.clone());
        }
        Ok(match &self.policy {
            HostKeyPolicy::AcceptAll | HostKeyPolicy::TrustOnFirstUse => true,
            HostKeyPolicy::Pinned { fingerprint: pinned } => pinned == &fingerprint,
        })
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        if let Ok(mut observed) = self.observed.lock() {
            observed.banner = Some(banner.to_string());
        }
        Ok(())
    }
}

enum Cmd {
    Write(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Eof,
    Close,
}

pub struct SshSession {
    pub id: Uuid,
    pub fingerprint: String,
    cmd_tx: mpsc::Sender<Cmd>,
    events: mpsc::Receiver<SshEvent>,
    handle: client::Handle<Handler>,
}

/// Cloneable command channel for a running session.
///
/// Tauri owns the `SshSession` in a background task (to stream events) while
/// the UI keeps an `SshHandle` for input/resize/close.
#[derive(Clone)]
pub struct SshHandle {
    cmd_tx: mpsc::Sender<Cmd>,
}

impl SshHandle {
    pub fn write(&self, data: impl Into<Vec<u8>>) {
        let _ = self.cmd_tx.try_send(Cmd::Write(data.into()));
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.cmd_tx.try_send(Cmd::Resize { cols, rows });
    }

    pub fn send_eof(&self) {
        let _ = self.cmd_tx.try_send(Cmd::Eof);
    }

    pub fn close(&self) {
        let _ = self.cmd_tx.try_send(Cmd::Close);
    }
}

impl SshSession {
    /// Cheap handle that can be stored next to a background event loop.
    pub fn handle(&self) -> SshHandle {
        SshHandle {
            cmd_tx: self.cmd_tx.clone(),
        }
    }

    pub fn write(&self, data: impl Into<Vec<u8>>) {
        let _ = self.cmd_tx.try_send(Cmd::Write(data.into()));
    }

    pub fn write_str(&self, data: &str) {
        self.write(data.as_bytes().to_vec());
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.cmd_tx.try_send(Cmd::Resize { cols, rows });
    }

    pub fn send_eof(&self) {
        let _ = self.cmd_tx.try_send(Cmd::Eof);
    }

    /// Non-blocking poll, used by the FFI bridge.
    pub fn try_event(&mut self) -> Option<SshEvent> {
        self.events.try_recv().ok()
    }

    pub async fn next_event(&mut self) -> Option<SshEvent> {
        self.events.recv().await
    }

    /// Politely closes the channel and the SSH transport.
    pub async fn request_close(&self) {
        let _ = self.cmd_tx.try_send(Cmd::Close);
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }

    pub async fn disconnect(self) {
        self.request_close().await;
    }
}

async fn authenticate(
    session: &mut client::Handle<Handler>,
    target: &SshTarget,
) -> Result<(), SshError> {
    let user = target.username.clone();
    match &target.auth {
        AuthMethod::Password { password } => {
            let ok = session
                .authenticate_password(user.clone(), password.clone())
                .await?
                .success();
            if !ok {
                return Err(SshError::AuthFailed(user));
            }
        }
        AuthMethod::KeyboardInteractive { password } => {
            // Some servers ask for password + OTP. We answer every prompt with
            // the password; the terminal remains available for follow-ups.
            let mut response = session
                .authenticate_keyboard_interactive_start(user.clone(), None::<String>)
                .await?;
            loop {
                match response {
                    client::KeyboardInteractiveAuthResponse::Success => break,
                    client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                        return Err(SshError::AuthFailed(user));
                    }
                    client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                        let answers = prompts
                            .iter()
                            .map(|p| {
                                if p.echo {
                                    String::new()
                                } else {
                                    password.clone()
                                }
                            })
                            .collect::<Vec<_>>();
                        response = session
                            .authenticate_keyboard_interactive_respond(answers)
                            .await?;
                    }
                }
            }
        }
        AuthMethod::Key {
            identity_id,
            passphrase,
        } => {
            let identity = target
                .identity
                .as_ref()
                .filter(|i| &i.id == identity_id)
                .ok_or(SshError::MissingIdentity)?;
            let pass = passphrase
                .clone()
                .or_else(|| identity.passphrase.clone());
            let key = decode_secret_key(&identity.private_key, pass.as_deref())
                .map_err(|e| SshError::InvalidKey(e.to_string()))?;
            let hash = session.best_supported_rsa_hash().await?.flatten();
            let ok = session
                .authenticate_publickey(
                    user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await?
                .success();
            if !ok {
                return Err(SshError::AuthFailed(user));
            }
        }
        AuthMethod::Agent => {
            // Agent forwarding is a desktop feature; SSH_AUTH_SOCK is read by
            // russh's agent client on unix hosts.
            #[cfg(unix)]
            {
                let sock = std::env::var("SSH_AUTH_SOCK")
                    .map_err(|_| SshError::AuthFailed(user.clone()))?;
                let mut agent = russh::keys::agent::client::AgentClient::connect_uds(sock)
                    .await
                    .map_err(|e| SshError::Agent(e.to_string()))?;
                let keys = agent
                    .request_identities()
                    .await
                    .map_err(|e| SshError::Agent(e.to_string()))?;
                let mut success = false;
                for identity in keys {
                    let public = identity.public_key().into_owned();
                    let result = session
                        .authenticate_publickey_with(
                            user.clone(),
                            public,
                            None,
                            &mut agent,
                        )
                        .await
                        .map_err(|e| SshError::Agent(e.to_string()))?;
                    if result.success() {
                        success = true;
                        break;
                    }
                }
                if !success {
                    return Err(SshError::AuthFailed(user));
                }
            }
            #[cfg(not(unix))]
            {
                return Err(SshError::AuthFailed(user));
            }
        }
    }
    Ok(())
}

/// Connects, authenticates, opens a PTY and a shell, then spawns the pump task.
pub async fn open_shell(target: SshTarget, size: TermSize) -> Result<SshSession, SshError> {
    let Authed {
        session,
        fingerprint,
        banner,
    } = connect_and_authenticate(&target).await?;

    let channel = session.channel_open_session().await?;
    for (name, value) in &target.env {
        let _ = channel.set_env(false, name.as_str(), value.as_str()).await;
    }
    let cols = size.cols.max(1);
    let rows = size.rows.max(1);
    channel
        .request_pty(false, &target.term, cols, rows, 0, 0, &[])
        .await?;
    channel.request_shell(false).await?;

    let (mut read, write) = channel.split();
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Cmd>(256);
    let (evt_tx, evt_rx) = mpsc::channel::<SshEvent>(1024);

    if let Some(banner) = banner {
        let _ = evt_tx.try_send(SshEvent::Banner { message: banner });
    }
    let _ = evt_tx.try_send(SshEvent::Connected {
        fingerprint: fingerprint.clone(),
    });

    let evt = evt_tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(Cmd::Write(data)) => {
                            if write.data_bytes(data).await.is_err() {
                                break;
                            }
                        }
                        Some(Cmd::Resize { cols, rows }) => {
                            let _ = write.window_change(cols, rows, 0, 0).await;
                        }
                        Some(Cmd::Eof) => {
                            let _ = write.eof().await;
                        }
                        Some(Cmd::Close) | None => {
                            let _ = write.eof().await;
                            let _ = write.close().await;
                            break;
                        }
                    }
                }
                msg = read.wait() => {
                    match msg {
                        Some(ChannelMsg::Data { data }) => {
                            if evt.send(SshEvent::Data { data_b64: B64.encode(&data) }).await.is_err() {
                                break;
                            }
                        }
                        Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                            let _ = evt.send(SshEvent::Stderr { data_b64: B64.encode(&data) }).await;
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => {
                            let _ = evt.send(SshEvent::ExitStatus { code: exit_status }).await;
                        }
                        Some(ChannelMsg::Close) | None => break,
                        Some(ChannelMsg::Eof) => {}
                        _ => {}
                    }
                }
            }
        }
        let _ = evt.send(SshEvent::Closed { reason: "channel closed".into() }).await;
    });

    Ok(SshSession {
        id: Uuid::new_v4(),
        fingerprint,
        cmd_tx,
        events: evt_rx,
        handle: session,
    })
}

/// Detects the remote OS family and release string from a live host.
///
/// Used to render an OS-aware glyph in the sidebar after the first successful
/// connection. Falls back to `uname` when `/etc/os-release` is not available.
pub async fn probe_os(target: SshTarget) -> Result<String, SshError> {
    let out = exec_once(
        target,
        "cat /etc/os-release 2>/dev/null || cat /usr/lib/os-release 2>/dev/null; \
         uname -s -m 2>/dev/null",
    )
    .await?;
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();

    // Prefer /etc/os-release fields. The value may be quoted.
    let mut os_id = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("id=") {
            os_id = Some(rest.trim_matches(|c| c == '"' || c == '\'').to_string());
            break;
        }
        if line.starts_with("name=") {
            let rest = line["name=".len()..].trim_matches(|c| c == '"' || c == '\'').to_string();
            if !rest.is_empty() && os_id.is_none() {
                os_id = Some(rest);
            }
        }
    }

    if let Some(id) = os_id {
        return Ok(normalize_os_id(&id));
    }

    if text.contains("darwin") { return Ok("macos".into()); }
    if text.contains("linux") { return Ok("linux".into()); }
    if text.contains("windows") || text.contains("mingw") || text.contains("cygwin") {
        return Ok("windows".into());
    }
    Ok("unknown".into())
}

fn normalize_os_id(raw: &str) -> String {
    let id = raw
        .to_lowercase()
        .replace(['"', '\''], "")
        .replace([' ', '-'], "_");
    let canonical = match id.as_str() {
        "ubuntu" | "kubuntu" | "lubuntu" | "xubuntu" | "zorin"
        | "neon" | "kubuntu_neon" => "ubuntu",
        "linuxmint" | "mint" => "mint",
        "pop_os" | "pop" => "pop",
        "elementary" => "elementary",
        "cachyos" => "cachyos",
        "debian" | "mx" | "mx_linux" => "debian",
        "raspbian" | "raspbian_armf" | "raspberrypi" => "raspbian",
        "kali" => "kali",
        "arch" | "archlinux" | "manjaro" | "endeavouros" | "artix" => "arch",
        "fedora" | "nobara" => "fedora",
        "rhel" | "red_hat" | "redhat" => "rhel",
        "centos" | "centos_stream" => "centos",
        "rocky" | "rockylinux" => "rocky",
        "almalinux" | "alma" => "alma",
        "ol" | "oracle" | "amzn" | "amazon" | "amazon_linux" | "amazonlinux"
        | "azure_linux" => "linux",
        "suse" | "sles" | "sle" | "sle_bci" | "opensuse" | "opensuse_leap"
        | "opensuse_tumbleweed" | "tumbleweed" | "leap" => "suse",
        "alpine" => "alpine",
        "gentoo" => "gentoo",
        "freebsd" => "freebsd",
        "macos" | "darwin" | "macosx" => "macos",
        "windows" | "windows_nt" | "windows_server" => "windows",
        "linux" => "linux",
        _ if id.contains("mint") => "mint",
        _ if id.contains("ubuntu") || id.contains("pop") => "ubuntu",
        _ if id.contains("elementary") => "elementary",
        _ if id.contains("kali") => "kali",
        _ if id.contains("raspbian") || id.contains("raspberry") => "raspbian",
        _ if id.contains("freebsd") => "freebsd",
        _ if id.contains("debian") => "debian",
        _ if id.contains("arch") || id.contains("manjaro") => "arch",
        _ if id.contains("fedora") => "fedora",
        _ if id.contains("centos") => "centos",
        _ if id.contains("rocky") => "rocky",
        _ if id.contains("alma") => "alma",
        _ if id.contains("red") => "rhel",
        _ if id.contains("suse") => "suse",
        _ if id.contains("alpine") => "alpine",
        _ if id.contains("gentoo") => "gentoo",
        _ if id.contains("darwin") || id.contains("macos") => "macos",
        _ if id.contains("windows") => "windows",
        other => return other.to_string(),
    };
    canonical.to_string()
}

#[derive(Debug, Clone, Default)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: Option<u32>,
}

/// One-shot command execution; handy for health checks and the CLI.
pub async fn exec_once(target: SshTarget, command: &str) -> Result<ExecOutput, SshError> {
    let fingerprint_slot = Arc::new(Mutex::new(Observed::default()));
    let handler = Handler {
        policy: target.host_key_policy.clone(),
        observed: fingerprint_slot.clone(),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });
    let addr = (target.host.as_str(), target.port);
    let mut session = tokio::time::timeout(target.timeout, client::connect(config, addr, handler))
        .await
        .map_err(|_| SshError::Timeout)??;
    if let HostKeyPolicy::Pinned { fingerprint } = &target.host_key_policy {
        let observed = fingerprint_slot.lock().expect("observed lock");
        if let Some(actual) = &observed.fingerprint {
            if fingerprint != actual {
                return Err(SshError::HostKeyMismatch {
                    expected: fingerprint.clone(),
                    actual: actual.clone(),
                });
            }
        }
    }
    authenticate(&mut session, &target).await?;

    let mut channel = session.channel_open_session().await?;
    channel.exec(true, command).await?;

    let mut out = ExecOutput::default();
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => out.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => out.exit_status = Some(exit_status),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    Ok(out)
}
