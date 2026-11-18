# Testing

## 1. Automated tests (Rust)

```bash
cd ~/Desktop/rterm/rust
cargo test --workspace
```

What `rterm-core` covers:

- `crypto` — key derivation, seal/open round-trip, distinct ciphertext every
  time (random nonce), wrong passphrase fails.
- `model` — per-entity LWW merge, tombstone wins over the older version, an
  entity present on only one side is kept.
- `bridge` / `drive` — sync client; network tests are skipped without credentials.
- `oauth` — id_token / account parsing.

Frontend:

```bash
cd ~/Desktop/rterm/desktop
npm run check          # svelte-check + TypeScript
npm run build          # production build (adapter-static)
```

## 2. Engine testing without the UI (CLI)

Set up a throwaway local sshd (macOS ships `/usr/sbin/sshd`):

```bash
mkdir -p /tmp/rterm-sshd && cd /tmp/rterm-sshd
ssh-keygen -q -t ed25519 -N '' -f host_ed25519
cp ~/.ssh/id_ed25519.pub authorized_keys 2>/dev/null || \
  ssh-keygen -q -t ed25519 -N '' -f id_ed25519 && cp id_ed25519.pub authorized_keys

cat > sshd_config <<'EOF'
Port 2222
ListenAddress 127.0.0.1
HostKey /tmp/rterm-sshd/host_ed25519
PidFile /tmp/rterm-sshd/sshd.pid
AuthorizedKeysFile /tmp/rterm-sshd/authorized_keys
PasswordAuthentication yes
UsePAM no
StrictModes no
EOF

sudo /usr/sbin/sshd -f /tmp/rterm-sshd/sshd_config -E /tmp/rterm-sshd/sshd.log
```

Then:

```bash
cd ~/Desktop/rterm/rust

# PTY shell
cargo run -p rterm-cli -- shell --host 127.0.0.1 --port 2222 \
  --user "$USER" --identity /tmp/rterm-sshd/id_ed25519

# one-shot command
cargo run -p rterm-cli -- exec --host 127.0.0.1 --port 2222 \
  --user "$USER" --identity /tmp/rterm-sshd/id_ed25519 -- uname -a

# host key pinning
cargo run -p rterm-cli -- exec --host 127.0.0.1 --port 2222 --user "$USER" \
  --identity /tmp/rterm-sshd/id_ed25519 --pin SHA256:wrong -- true   # must be rejected

# generate a key
cargo run -p rterm-cli -- keygen --out /tmp/rterm-sshd/generated
```

Tear down afterwards: `sudo kill $(cat /tmp/rterm-sshd/sshd.pid)`.

## 3. Manual app checklist

Run `cd desktop && npm run tauri dev`.

### Onboarding

- [ ] The landing screen shows **Sign in with Google** and
  **Use without Google**.
- [ ] Login opens the browser; after consent the window returns and the email shows.
- [ ] On first sign-in: the **Create vault** screen appears.
- [ ] A passphrase shorter than 8 characters is rejected.
- [ ] A mismatched confirmation is rejected.
- [ ] After creation the vault screen opens; `vault.json` appears in
  `~/Library/Application Support/app.rterm/`.

### Vault & hosts

- [ ] Add a host with a password; it is saved and shows in the sidebar.
- [ ] Close and reopen the app: auto-unlock from the keychain (no passphrase).
- [ ] On a second device (empty data folder): Google sign-in → **Retrieve from
  the sync bridge** → same passphrase → the vault and its hosts appear.
- [ ] **Lock vault** → the unlock screen appears; a wrong passphrase shows a
  clear error.
- [ ] Edit a host (⋯ button or double-click) → changes persist.
- [ ] Delete a host → it disappears from the list (the tombstone stays in the JSON).
- [ ] Generate an Ed25519 key from the host form → a new identity appears in the picker.

### Terminal

- [ ] Click a host → a tab opens, status `connecting` → `connected`.
- [ ] `motd`/prompt output shows, input echo is correct.
- [ ] The `[host key SHA256:…]` line appears on first connect, and
  `host_key_fingerprint` is saved into the vault.
- [ ] The second connection no longer prints the host key (already pinned).
- [ ] Resizing the window → `ssh_resize` recomputes (programs like `vim`
  adapt).
- [ ] `top`, `vim`, 256/truecolor colours render correctly.
- [ ] Closing a tab terminates the session (check `ps` on the server).

### Sync

- [ ] **Sync now** → a notification showing the new revision.
- [ ] Open the app on a second device with the same client ID + account +
  passphrase → hosts appear.
- [ ] Edit a host on device A, then sync; sync on device B; the newest change wins.
- [ ] Delete a host on A, sync; sync on B; the host does not come back.
- [ ] Add a host offline on B (no sign-in), sync after signing in → merge, not overwrite.

### File security

```bash
cd ~/Library/Application\ Support/app.rterm
jq '.vault.hosts[0] | {label, address, username, auth}' vault.json
```

- [ ] `label`, `address`, `username`, `tags` are readable in plaintext.
- [ ] `auth.Password.password` is shaped `enc:v1:<nonce>:<ciphertext>`.
- [ ] `kdf.alg` = `argon2id`; `canary` is also `enc:v1:…`.
- [ ] In D1 (`wrangler d1 execute rterm-sync --remote --command "SELECT blob FROM vaults"`), the document carries `kdf` + `canary` (not secret) and the
  vault's secrets remain `enc:v1:…`.
- [ ] `jq '.vault.hosts[0].auth' vault.json` does not leak the password.
- [ ] Wrong passphrase → clear error; the file is unchanged.
- [ ] `vault.json.bak` exists after the second save.
- [ ] `security find-generic-password -s app.rterm` shows the keychain item.

### After sign-in

- [ ] Restart the app → Google sign-in is restored without a browser (from the keychain).
- [ ] **Sign out** in Settings → `auth_restore` no longer succeeds.
- [ ] Settings shows the vault location, device ID, and version.

## 4. Reset

```bash
# local (vault + settings)
rm -rf ~/Library/Application\ Support/app.rterm

# keychain
security delete-generic-password -s app.rterm -a vault-key 2>/dev/null
security delete-generic-password -s app.rterm -a sync-refresh 2>/dev/null
security delete-generic-password -s app.rterm -a oauth-refresh 2>/dev/null
security delete-generic-password -s app.rterm -a account 2>/dev/null

# D1 document (via wrangler; ciphertext only)
# wrangler d1 execute rterm-sync --remote --command "DELETE FROM users WHERE email = 'you@example.com'"
# (cascade: devices + refresh_tokens + vaults)
```

## 5. Regressions to watch

- Never store a plaintext password/private key in `vault.json`.
- `seal_str` must be idempotent: values already marked `enc:v1:` are not sealed
  twice (otherwise sync would double-encrypt ciphertext).
- `ssh_open` must accept the `session_id` from the UI; do not go back to the
  "Rust picks the id" pattern — the frontend listener would register too late.
- `vault_sync` must use `JsonSyncDoc` + revision CAS, not a whole-file envelope.
- File writes must remain atomic (tmp + fsync + rename) and keep the
  `.bak`.
