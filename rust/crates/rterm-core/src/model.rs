//! Domain model for the encrypted vault.
//!
//! Everything in here is *client side* data: it is serialised to JSON, then
//! encrypted with the user's vault key before it ever reaches the sync server.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub type Id = Uuid;

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// How we authenticate to a host.
///
/// Secrets live inside the encrypted vault, never on the sync server in clear.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    Password {
        password: String,
    },
    /// Reference to an `Identity` stored in the vault.
    Key {
        identity_id: Id,
        passphrase: Option<String>,
    },
    Agent,
    /// Password based "keyboard-interactive" (2FA prompts get the password).
    KeyboardInteractive {
        password: String,
    },
}

impl Default for AuthMethod {
    fn default() -> Self {
        AuthMethod::Password {
            password: String::new(),
        }
    }
}

// F-12/R-05: credentials must never leak through a derived `Debug`.
impl std::fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password { .. } => f.write_str("Password { password: [REDACTED] }"),
            Self::Key { identity_id, passphrase } => f
                .debug_struct("Key")
                .field("identity_id", identity_id)
                .field("passphrase", &passphrase.as_ref().map(|_| "[REDACTED]"))
                .finish(),
            Self::Agent => f.write_str("Agent"),
            Self::KeyboardInteractive { .. } => {
                f.write_str("KeyboardInteractive { password: [REDACTED] }")
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum IdentityKind {
    Ed25519,
    Rsa,
    Ecdsa,
    Imported,
}

/// A reusable SSH key.
#[derive(Serialize, Deserialize, Clone)]
pub struct Identity {
    pub id: Id,
    pub label: String,
    /// OpenSSH / PEM / PPK private key, encrypted at rest by the vault itself.
    pub private_key: String,
    /// Passphrase of the private key (not of the vault).
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub kind: Option<IdentityKind>,
    /// `SHA256:...` of the public key, filled when the key is imported/generated.
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub deleted: bool,
}

// F-12/R-05: private key material and its passphrase are never printed.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("private_key", &"[REDACTED]")
            .field("passphrase", &self.passphrase.as_ref().map(|_| "[REDACTED]"))
            .field("kind", &self.kind)
            .field("fingerprint", &self.fingerprint)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("deleted", &self.deleted)
            .finish()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Group {
    pub id: Id,
    pub label: String,
    #[serde(default)]
    pub parent_id: Option<Id>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Snippet {
    pub id: Id,
    pub label: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub deleted: bool,
}

/// A free-form note. The title shown in lists is derived from the first words
/// of `body`, so a note stores only its text.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Note {
    pub id: Id,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TerminalPrefs {
    /// TERM value, e.g. `xterm-256color`.
    #[serde(default)]
    pub term: Option<String>,
    #[serde(default)]
    pub font_size: Option<u16>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub scrollback: Option<u32>,
    #[serde(default)]
    pub cursor_blink: Option<bool>,
    #[serde(default)]
    pub theme: Option<String>,
}

/// A saved server.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Host {
    pub id: Id,
    #[serde(default)]
    pub label: String,
    pub address: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth: AuthMethod,

    #[serde(default)]
    pub group_id: Option<Id>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub favorite: bool,

    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub startup_snippet_id: Option<Id>,
    /// Another host used as a jump/proxy host (`ssh -J`).
    #[serde(default)]
    pub jump_host_id: Option<Id>,
    #[serde(default)]
    pub keepalive_secs: Option<u64>,
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    /// Extra environment variables requested on the session.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub terminal: TerminalPrefs,
    #[serde(default)]
    pub agent_forwarding: bool,
    #[serde(default)]
    pub compression: bool,

    /// Detected OS family on the remote host, e.g. `ubuntu`, `macos`, `windows`.
    /// Filled by a one-shot probe after the first successful connection.
    #[serde(default)]
    pub os: Option<String>,

    /// Pinned `SHA256:...` host key (TOFU when empty).
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,

    #[serde(default)]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub deleted: bool,
}

fn default_port() -> u16 {
    22
}

impl Host {
    pub fn display_name(&self) -> &str {
        if self.label.is_empty() {
            &self.address
        } else {
            &self.label
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    Local,
    Remote,
    Dynamic,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PortForward {
    pub id: Id,
    pub host_id: Id,
    pub kind: ForwardKind,
    pub bind_host: String,
    pub bind_port: u16,
    pub target_host: String,
    pub target_port: u16,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VaultSettings {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default = "default_term")]
    pub default_term: String,
    #[serde(default = "default_font_size")]
    pub font_size: u16,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default = "default_scrollback")]
    pub scrollback: u32,
    #[serde(default)]
    pub sync_interval_secs: u64,
    /// Auto lock the vault after N seconds without activity (0 = never).
    #[serde(default = "default_lock_timeout")]
    pub lock_timeout_secs: u64,
    #[serde(default)]
    pub copy_on_select: bool,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

fn default_term() -> String {
    "xterm-256color".into()
}
fn default_font_size() -> u16 {
    13
}
fn default_scrollback() -> u32 {
    10_000
}
fn default_lock_timeout() -> u64 {
    300
}

impl Default for VaultSettings {
    fn default() -> Self {
        Self {
            theme: None,
            default_term: default_term(),
            font_size: default_font_size(),
            font_family: None,
            scrollback: default_scrollback(),
            sync_interval_secs: 60,
            lock_timeout_secs: default_lock_timeout(),
            copy_on_select: false,
            updated_at: None,
        }
    }
}

/// The whole user data set. Serialised, encrypted, synced as one blob.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Vault {
    #[serde(default = "schema_version")]
    pub schema: u32,
    #[serde(default)]
    pub hosts: Vec<Host>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub identities: Vec<Identity>,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
    #[serde(default)]
    pub settings: VaultSettings,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

fn schema_version() -> u32 {
    1
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            schema: 1,
            hosts: Vec::new(),
            groups: Vec::new(),
            identities: Vec::new(),
            snippets: Vec::new(),
            notes: Vec::new(),
            port_forwards: Vec::new(),
            settings: VaultSettings::default(),
            updated_at: None,
        }
    }
}

impl Vault {
    pub fn identity(&self, id: &Id) -> Option<&Identity> {
        self.identities.iter().find(|i| &i.id == id && !i.deleted)
    }

    pub fn host(&self, id: &Id) -> Option<&Host> {
        self.hosts.iter().find(|h| &h.id == id && !h.deleted)
    }

    pub fn live_hosts(&self) -> impl Iterator<Item = &Host> {
        self.hosts.iter().filter(|h| !h.deleted)
    }

    /// Merge `remote` into `self` using last-writer-wins per live entity.
    /// A tombstone (`deleted = true`) is terminal: it wins over any live record
    /// regardless of `updated_at`, so a clock-skewed older client or a replayed
    /// snapshot can never resurrect a deleted secret (R-04). A future undelete
    /// feature must mint a NEW id instead of clearing a tombstone in place.
    pub fn merge(&mut self, remote: &Vault) {
        merge_vec(&mut self.hosts, remote.hosts.clone());
        merge_vec(&mut self.groups, remote.groups.clone());
        merge_vec(&mut self.identities, remote.identities.clone());
        merge_vec(&mut self.snippets, remote.snippets.clone());
        merge_vec(&mut self.notes, remote.notes.clone());
        merge_vec(&mut self.port_forwards, remote.port_forwards.clone());

        let remote_settings_at = remote.settings.updated_at;
        let local_settings_at = self.settings.updated_at;
        if remote_settings_at > local_settings_at {
            self.settings = remote.settings.clone();
        }

        if remote.updated_at > self.updated_at {
            self.updated_at = remote.updated_at;
        }
        self.touch();
    }

    /// Marks the vault as modified now.
    pub fn touch(&mut self) {
        self.updated_at = Some(now());
    }

    /// Remove the payload of old tombstones but KEEP the tombstone itself.
    /// Dropping the record entirely lets an offline replica replay the older
    /// live entity and resurrect a deleted secret (F-39).
    pub fn compact_tombstones(&mut self, days: i64) {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        for host in self.hosts.iter_mut().filter(|h| h.deleted && h.updated_at <= cutoff) {
            host.auth = AuthMethod::Agent;
            host.notes.clear();
            host.tags.clear();
            host.label.clear();
            host.address.clear();
        }
        for identity in self.identities.iter_mut().filter(|i| i.deleted && i.updated_at <= cutoff) {
            identity.private_key.clear();
            identity.passphrase = None;
            identity.label.clear();
        }
        for snippet in self.snippets.iter_mut().filter(|s| s.deleted && s.updated_at <= cutoff) {
            snippet.content.clear();
            snippet.label.clear();
        }
        for note in self.notes.iter_mut().filter(|n| n.deleted && n.updated_at <= cutoff) {
            note.body.clear();
        }
        for group in self.groups.iter_mut().filter(|g| g.deleted && g.updated_at <= cutoff) {
            group.label.clear();
        }
        // R-04: port-forward tombstones were skipped, leaving bind/target hosts
        // (which can be internal hostnames) in old synced tombstones.
        for pf in self.port_forwards.iter_mut().filter(|p| p.deleted && p.updated_at <= cutoff) {
            pf.label = None;
            pf.bind_host.clear();
            pf.target_host.clear();
            // id / kind / ports stay so the tombstone remains a valid record.
        }
    }

    /// All tags in use, sorted and de-duplicated.
    pub fn tags(&self) -> Vec<String> {
        let mut set: HashSet<String> = HashSet::new();
        for h in self.live_hosts() {
            for t in &h.tags {
                set.insert(t.clone());
            }
        }
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    }
}

pub trait SyncItem {
    fn id(&self) -> Id;
    fn updated_at(&self) -> DateTime<Utc>;
    fn deleted(&self) -> bool;
}

macro_rules! sync_item {
    ($ty:ty) => {
        impl SyncItem for $ty {
            fn id(&self) -> Id {
                self.id
            }
            fn updated_at(&self) -> DateTime<Utc> {
                self.updated_at
            }
            fn deleted(&self) -> bool {
                self.deleted
            }
        }
    };
}

sync_item!(Host);
sync_item!(Group);
sync_item!(Identity);
sync_item!(Snippet);
sync_item!(Note);
sync_item!(PortForward);

/// Whether `incoming` should replace `current` for the same entity id.
/// Deletes dominate `updated_at`: a skewed clock or a replayed snapshot must
/// never resurrect a secret that was deliberately deleted (R-04).
fn should_replace<T: SyncItem>(current: &T, incoming: &T) -> bool {
    match (current.deleted(), incoming.deleted()) {
        (true, false) => false, // tombstone is terminal
        (false, true) => true,  // delete wins even if its clock is behind
        _ => incoming.updated_at() > current.updated_at(),
    }
}

fn merge_vec<T: SyncItem + Clone>(local: &mut Vec<T>, remote: Vec<T>) {
    let mut index: HashMap<Id, usize> = local
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id(), i))
        .collect();

    for item in remote {
        match index.get(&item.id()) {
            Some(&i) => {
                if should_replace(&local[i], &item) {
                    local[i] = item;
                }
            }
            None => {
                index.insert(item.id(), local.len());
                local.push(item);
            }
        }
    }
}

/// Public-facing, non-secret view of a saved host (used by list UIs).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HostSummary {
    pub id: Id,
    pub label: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub tags: Vec<String>,
    pub group_id: Option<Id>,
    pub color: Option<String>,
    pub favorite: bool,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<&Host> for HostSummary {
    fn from(h: &Host) -> Self {
        Self {
            id: h.id,
            label: h.display_name().to_string(),
            address: h.address.clone(),
            port: h.port,
            username: h.username.clone(),
            tags: h.tags.clone(),
            group_id: h.group_id,
            color: h.color.clone(),
            favorite: h.favorite,
            last_used_at: h.last_used_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: Id, label: &str, updated_at: DateTime<Utc>) -> Host {
        Host {
            id,
            label: label.into(),
            address: "example.com".into(),
            port: 22,
            username: "root".into(),
            auth: AuthMethod::default(),
            group_id: None,
            tags: vec![],
            color: None,
            icon: None,
            favorite: false,
            notes: String::new(),
            startup_snippet_id: None,
            jump_host_id: None,
            keepalive_secs: None,
            connect_timeout_secs: None,
            env: vec![],
            terminal: TerminalPrefs::default(),
            agent_forwarding: false,
            compression: false,
            os: None,
            host_key_fingerprint: None,
            created_at: Utc::now(),
            updated_at,
            last_used_at: None,
            deleted: false,
        }
    }

    #[test]
    fn newer_remote_wins() {
        let id = Uuid::new_v4();
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::seconds(5);
        let mut local = Vault::default();
        local.hosts.push(host(id, "old", t0));
        let mut remote = Vault::default();
        remote.hosts.push(host(id, "new", t1));
        local.merge(&remote);
        assert_eq!(local.hosts.len(), 1);
        assert_eq!(local.hosts[0].label, "new");
    }

    #[test]
    fn tombstone_wins_when_newest() {
        let id = Uuid::new_v4();
        let t0 = Utc::now();
        let mut tombstone = host(id, "gone", t0 + chrono::Duration::seconds(1));
        tombstone.deleted = true;
        let mut local = Vault::default();
        local.hosts.push(host(id, "alive", t0));
        let mut remote = Vault::default();
        remote.hosts.push(tombstone);
        local.merge(&remote);
        assert!(local.hosts[0].deleted);
    }

    #[test]
    fn tombstone_wins_equal_timestamp_and_compaction_keeps_it() {
        let id = Uuid::new_v4();
        let t0 = Utc::now() - chrono::Duration::days(400);
        let mut tombstone = host(id, "gone", t0);
        tombstone.deleted = true;
        tombstone.notes = "a secret".into();
        let mut local = Vault::default();
        local.hosts.push(host(id, "alive", t0));
        let mut remote = Vault::default();
        remote.hosts.push(tombstone);
        local.merge(&remote);
        assert!(local.hosts[0].deleted);
        local.compact_tombstones(30);
        assert_eq!(local.hosts.len(), 1, "tombstone must survive compaction");
        assert!(local.hosts[0].notes.is_empty(), "payload must be stripped");
    }

    #[test]
    fn skewed_live_record_cannot_resurrect_tombstone() {
        let id = Uuid::new_v4();
        let t0 = Utc::now();
        let mut tombstone = host(id, "gone", t0);
        tombstone.deleted = true;
        tombstone.notes = "secret".into();
        let mut local = Vault::default();
        local.hosts.push(tombstone);
        let mut remote = Vault::default();
        // A clock-skewed replica's live record is one day newer.
        remote.hosts.push(host(id, "resurrected", t0 + chrono::Duration::days(1)));
        local.merge(&remote);
        assert_eq!(local.hosts.len(), 1);
        assert!(local.hosts[0].deleted, "tombstone must remain terminal (R-04)");
        assert_eq!(local.hosts[0].label, "gone");
        assert_eq!(local.hosts[0].notes, "secret");
    }

    #[test]
    fn older_tombstone_still_deletes_newer_live_record() {
        let id = Uuid::new_v4();
        let t0 = Utc::now();
        let mut local = Vault::default();
        local.hosts.push(host(id, "alive", t0 + chrono::Duration::days(1)));
        let mut tombstone = host(id, "gone", t0);
        tombstone.deleted = true;
        let mut remote = Vault::default();
        remote.hosts.push(tombstone);
        local.merge(&remote);
        assert!(local.hosts[0].deleted, "a delete must win even when its clock is behind (R-04)");
    }

    #[test]
    fn port_forward_tombstone_is_compacted() {
        let id = Uuid::new_v4();
        let t0 = Utc::now() - chrono::Duration::days(400);
        let mut pf = PortForward {
            id,
            host_id: Uuid::new_v4(),
            kind: ForwardKind::Local,
            bind_host: "127.0.0.1".into(),
            bind_port: 8080,
            target_host: "internal.example".into(),
            target_port: 22,
            label: Some("old forward".into()),
            updated_at: t0,
            deleted: true,
        };
        pf.deleted = true;
        let mut vault = Vault::default();
        vault.port_forwards.push(pf);
        vault.compact_tombstones(30);
        assert_eq!(vault.port_forwards.len(), 1, "tombstone must survive");
        assert!(vault.port_forwards[0].label.is_none());
        assert!(vault.port_forwards[0].bind_host.is_empty());
        assert!(vault.port_forwards[0].target_host.is_empty());
    }
}
