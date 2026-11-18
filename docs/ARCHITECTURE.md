# Architecture

## Layers

```
┌──────────────────────────────────────────────────────────┐
│ Svelte 5 UI (desktop/src)                                │
│  Welcome → Unlock/Setup → VaultShell → Terminal/Settings │
└───────────────▲──────────────────────────────────────────┘
                │ invoke(command)        event: ssh://<id>
┌───────────────┴──────────────────────────────────────────┐
│ Tauri command layer (desktop/src-tauri)                  │
│  auth · vault · sync · ssh · keychain · atomic JSON      │
└───────────────▲──────────────────────────────────────────┘
                │ crate path dependency
┌───────────────┴──────────────────────────────────────────┐
│ rterm-core (Rust)                                        │
│  model (Vault/merge) · crypto · ssh (russh) · bridge · oauth│
└──────────────────────────────────────────────────────────┘
```

There is no FFI/JNI/ObjC bridge: the frontend calls Rust directly through
Tauri, so `rterm-ffi` (a React Native-era artifact) has been removed.

## State in the Rust process

`desktop/src-tauri/src/state.rs`

| Field | Contents |
| --- | --- |
| `paths` | `vault.json` + `settings.json` under `app_data_dir` (`~/Library/Application Support/app.rterm` on macOS) |
| `vault` | `Vault` **plaintext** while open (secrets already decrypted) |
| `key` | Vault `SecretKey` (32 bytes) derived with Argon2id |
| `kdf` | `KdfParams` from the file, used when rewriting |
| `session` | access token, refresh token, Google account, `expires_at` |
| `ssh` | `HashMap<session_id, SshHandle>` for input/resize/close |

The file never stores plaintext secrets; `vault.json` always contains the
output of `secrets::seal_vault()`.

## Vault schema

The source of truth lives in `rust/crates/rterm-core/src/model.rs` (serde).
Summary:

- `Vault { schema, hosts, groups, identities, snippets, port_forwards, settings, updated_at }`
- `Host { id, label, address, port, username, auth, group_id, tags, color, icon,
  favorite, notes, startup_snippet_id, jump_host_id, keepalive_secs,
  connect_timeout_secs, env, terminal, agent_forwarding, compression,
  host_key_fingerprint, created_at, updated_at, last_used_at, deleted }`
- `AuthMethod`: `Password`, `Key { identity_id, passphrase }`, `Agent`,
  `KeyboardInteractive`
- `Identity { id, label, private_key, passphrase, kind, fingerprint, … }`
- All entities carry `updated_at` + `deleted` for merge.

### Merge (LWW + tombstone)

`Vault::merge(&other)` compares entity by entity on `id`:

- present on only one side → keep it
- present on both → the newer `updated_at` wins
- `deleted: true` beats a non-deleted version when it is newer,
  so deletions do not "come back to life" after sync.

Because secrets are sealed before merging, ciphertext rides along with its
metadata — merging requires no key.

## Field-level encryption

`desktop/src-tauri/src/secrets.rs`

```
enc:v1:<nonce_b64>:<ciphertext_b64>
```

- XChaCha20-Poly1305, 24-byte random nonce per value.
- Key = the vault `SecretKey`; no AAD.
- Sealed: host passwords, keyboard-interactive passwords, `private_key`,
  `passphrase`. Everything else stays readable so the file can be `jq`-ed,
  diffed and reviewed.
- The `rterm-canary-v1` canary is sealed in the local file to detect a wrong
  passphrase without decrypting the whole vault.

### Key handling & keychain

```
passphrase ──Argon2id(salt, m=64MiB, t=3, p=1)──► SecretKey(32B)
                                                   │
                     ┌─────────────────────────────┴──────────────┐
                     ▼                                            ▼
         OS keychain: service "app.rterm",          seal each field
         account "vault-key"                       (enc:v1:…)
```

Google refresh token: keychain `account = "oauth-refresh"`, profile account at
`account = "account"`. The access token lives only in memory and is refreshed
automatically 60 seconds before expiry.

## Local storage

`desktop/src-tauri/src/store.rs`

1. `vault.json.bak` ← copy of the old file
2. write `vault.json.tmp` + `sync_all()`
3. `rename(tmp → vault.json)`

`settings.json` stores `google_client_id`, `bridge_url`, and `device_id`
(a per-install UUID). `bridge_url` may be empty — when it is, sync is disabled
(local mode keeps working); official releases inject it via `RTERM_BRIDGE_URL`
at build time.

## Bridge sync (Worker + D1)

`rterm-core::bridge::BridgeClient` + `bridge/` (Cloudflare Worker, TypeScript)

1. Sign-in: OAuth PKCE + loopback as usual, but the `code` goes to
   `POST /v1/auth/google` (not to Google). The bridge exchanges it with Google
   server-side and issues a RuTerm session (HS256 JWT, 1 hour + a rotating
   refresh token; SHA-256 hash in D1).
2. `fetch_json()` — `GET /v1/vault` → `{ revision, blob, updated_at }`.
   `revision == 0` or an empty `blob` → no vault yet.
3. `sync_json(local)`:
   - `merged = local.vault.merge(&remote.vault)` (LWW)
   - `PUT /v1/vault` with `base_revision`; `409` → refetch and retry
     (max 4 attempts).
4. The merged result is written atomically locally, then the plaintext
   `Vault` replaces the in-memory state.

The D1 blob is a `JsonSyncDoc` (`kdf` + `canary` + a vault with `enc:v1:`
sealed secrets). Neither is secret (the salt only defeats precomputation, the
canary is already ciphertext), and their presence lets a **new device** derive
the same key from the same passphrase and then open the vault via
`vault_adopt_remote`.

The vault key itself never syncs — the passphrase **must be identical** on every
device. If it differs, `open_vault` fails and sync is aborted without touching
local data. The server stores only ciphertext + revision — not even the operator
can read the contents. Deploy: `bridge/README.md`, details: `docs/SYNC.md`.

## SSH sessions

`rterm-core::ssh`

- `open_shell(target, size)` performs TCP + handshake + auth, opens a PTY
  channel, then returns an `SshSession`.
- `SshSession` owns the event receiver; the UI cannot hold it directly, so
  `session.handle()` yields an `SshHandle` (a cloneable `mpsc::Sender`) for
  `write` / `resize` / `close`.
- The `ssh_open` command stores the handle in state, then `spawn`s a task that
  reads `next_event()` and emits `SshEvent` on the `ssh://<session_id>` channel.
- The frontend creates the `session_id` (UUID) **before** calling `ssh_open`
  so `listen()` is registered before the first banner arrives — this avoids
  losing output on fast connections.
- Events: `connected {fingerprint}`, `banner`, `data`, `stderr`, `error`,
  `exit_status`, `closed`. Terminal payloads are base64 so partial UTF-8 is safe.
- TOFU: `connected.fingerprint` is stored into `host.host_key_fingerprint` by
  the UI; subsequent connections use `HostKeyPolicy::Pinned`.

## Tauri commands

| Command | Purpose |
| --- | --- |
| `app_status` | summary: vault present/open, account, device id |
| `app_version` | crate version |
| `auth_sign_in` | OAuth PKCE + loopback, stores the refresh token |
| `auth_restore` | silent login from the keychain |
| `auth_sign_out` | deletes the tokens |
| `vault_create` | new KDF, canary, writes an empty vault |
| `vault_unlock` | derive key + verify the canary |
| `vault_try_auto_unlock` | opens with the keychain key |
| `vault_lock` | forget the in-memory key + vault |
| `vault_get` | fetch the plaintext vault |
| `vault_save` | seal + atomic write + bump the revision |
| `vault_sync` | merge + push to the bridge |
| `ssh_open` / `ssh_write` / `ssh_resize` / `ssh_close` | session lifecycle |
| `ssh_keygen` | generate Ed25519/ECDSA keys into the vault |

## Design decisions

- **One JSON document** = one unit of sync. No per-host files, no database.
  Easy to back up, easy to inspect.
- **Field-level**, not whole-file encryption: the host structure stays
  readable (auditing, `jq`) while credentials remain ciphertext.
- **One OAuth client** of type *Desktop app* with PKCE + loopback; the client
  secret Google demands at the token endpoint is kept in the bridge
  (`wrangler secret`), never in the app or the repository.
- **Desktop first**: Tauri on macOS only needs Command Line Tools.
