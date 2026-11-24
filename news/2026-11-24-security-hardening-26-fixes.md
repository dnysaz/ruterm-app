# Security hardening: 26 fixes in one pass

A full audit of the webview, IPC commands, vault sync and release pipeline turned into 26 concrete fixes — 4 High, 12 Medium and 10 Low. Your everyday workflow stays the same; the point of this pass is to make sure secrets stay inside RuTerm and remote data is verified before it is trusted.

## What changed

- **The webview policy is back on.** Content Security Policy is active again (inline styles are still allowed so themes render correctly), `withGlobalTauri` is off, and the main vault window and the About window now have separate, minimal capabilities.
- **Sync documents are signed.** Every upload carries a key-derived HMAC over the whole envelope — vault fields, revision, settings, tombstones and timestamps. Missing or invalid signatures are rejected before anything is merged.
- **Local-only settings stay local.** AI chat permissions can no longer be switched on by a synced document, even a correctly signed one.
- **Sign-out really signs out.** It now performs the same full wipe as locking: key, plaintext vault, session tokens, pending keys, chat state and live SSH/SFTP/local sessions are all cleared. The local vault is tied to the Google account that owns it.
- **Dangerous commands are denied by default.** The MCP gate now understands nested shells, interpreters, `xargs`, `find -exec`, redirections and substitutions. Only a small read-only allowlist passes without asking.
- **Approvals come from a native dialog.** Approval IDs are no longer sent to the webview, so injected page code cannot answer its own prompt. Credential reads never offer “Always allow”; scoped approvals expire with the assistant reply.
- **The loopback bridge token stays in Rust.** Agents receive it through a private 0600 config file instead of the renderer. Loopback HTTP now validates `Host`/`Origin` and sends restrictive security headers.
- **Private reads ask first.** Notes, snippets and host notes now require an explicit approval before an AI tool can read them.
- **Chat history is redacted and wiped.** Every stored bubble is scrubbed before `localStorage`, and history is cleared on lock, sign-out and account change.
- **Release notes and installers are verified.** The in-app News feed is pinned to an immutable commit and each entry is SHA-256 checked; installers now fail closed when `SHA256SUMS` is missing, signing is enabled when Apple secrets are configured, and GitHub Actions are pinned to commit SHAs.
- **Logs and temp files are tightened.** MCP/chat logs use 0700 directories, 0600 atomic writes and secret redaction; raw `send_keys` content is no longer logged. Orphaned vault temp files are cleaned at startup and stale backups are deleted on reset.
- **Smaller path and env fixes.** `local_open` accepts only a strict env allowlist, SSH private keys are read by Rust via a native picker token, external links require HTTPS, dev hosts default to loopback, and `vault_lock` closes live sessions. Sanitizer `class`/`id` and remote images are blocked, SFTP upload handles are bound to their tab, and SSH config writes use `O_NOFOLLOW` with atomic 0600 renames.

## Things to know

- **Sync compatibility:** update every device before syncing again. Older unsigned remote documents are intentionally rejected; restore a signed copy or reset and re-adopt explicitly.
- **Legacy passphrase:** `12345678` still unlocks a local vault once and re-seals it with the current context. After migration the old fallback is disabled, and remote adopt/sync never uses it.
- **Reset is local-only:** it unlinks Google sync and deletes the local backup, but the remote copy stays untouched until you re-adopt it.
