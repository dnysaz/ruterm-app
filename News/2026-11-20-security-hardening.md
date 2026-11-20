# A tougher app under the hood

Most of this update is invisible. It closes a long list of ways things could go
wrong — lost keys, crossed wires between tabs, and secrets ending up in logs.

## Your saved unlock key won't vanish

If a saved key ever fails to open the vault because of a temporary error, the app
used to assume there was no key at all and delete it. It now checks whether the
key is actually there first, and only clears the setting when it really is gone.
A genuine wrong passphrase still tells you so instead of silently forgetting you.

## SFTP tabs mind their own business

Every SFTP tab now gets its own Rust-issued capability token, the same way
terminal sessions already did. Listing, reading, writing, uploading, renaming,
deleting and closing all require that token — one tab can no longer drive
another tab's session just by knowing its id.

File listings are also built locally now. A server can no longer hand us a
friendly name like `notes.txt` while quietly pointing it at a different file;
the path is constructed from the folder you actually opened, and odd names
(`..`, separators, hidden control characters) are refused.

## Deletes stay bounded, deletions stay deleted

- Recursive folder deletion has a 10,000-entry budget and a depth limit, so a
  hostile or broken server can't make the app chew through an endless tree.
- A delete now wins over a conflicting edit regardless of clock skew. Previously
  a device with a fast or slow clock could resurrect a secret you had removed.
- Old port-forward tombstones are cleaned up properly, like every other entity.

## Secrets stay out of logs

Passwords, private keys and key passphrases now redact themselves when Rust
prints them for debugging. Exported vault/settings JSON is written with owner-
only permissions instead of the system default. Remote vaults are structurally
validated before they touch disk, both when adopting a synced vault and after a
sync merge.

## Sync and sign-in got steadier

- Concurrent token refresh can no longer leave an orphaned refresh token behind.
- Rate limiting no longer trusts a client-supplied forwarding header.
- A malformed vault size limit fails closed instead of turning the limit off.
- Google sign-in responses that aren't valid JSON are handled instead of
  producing an unhandled error.
- CORS now only accepts origins you explicitly configure.

## We also fixed our wording

The bridge sync is **credential-encrypted**, not zero-knowledge: passwords,
private keys and secret note fields are sealed on your device, but host metadata
such as addresses and usernames is plain JSON on the server. The old
"even the operator cannot read the contents" claim was simply not true, and has
been corrected everywhere it appeared.

---

*Nothing here changes how you use RuTerm day to day. If you notice anything
unexpected after updating, please tell us.*
