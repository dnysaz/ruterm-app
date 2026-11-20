# Sync architecture — RuTerm bridge (Cloudflare Worker + D1)

RuTerm works fully offline with a local `vault.json`. Signing in with Google
adds cross-device sync through a **credential-encrypted bridge** you deploy yourself:

```
Desktop app ──loopback+PKCE──► Google (consent, your Desktop client ID)
     │
     └── code + code_verifier + redirect_uri ──► https://<bridge-domain>/v1/auth/google
                                                      │
                                             Cloudflare Worker (TypeScript)
                                                      │ exchanges code ➜ Google (server-side)
                                                      │ issues RuTerm JWT (HS256, 1 h)
                                                      │ + rotating refresh token (hash in D1)
                                                      ▼
                                            D1 (SQLite): users / devices /
                                            refresh_tokens / vaults (ciphertext only)
```

The app never holds a Google client secret and never sees Google tokens;
the one Desktop client secret Google requires at the token endpoint lives
in the Worker only (`wrangler secret put GOOGLE_CLIENT_SECRET`).
the bridge does not see plaintext — the vault blob is field-level encrypted
on-device before upload (XChaCha20-Poly1305, Argon2id).

## Why this shape

- No VPS to patch or pay for idle time; the Worker + D1 free tier covers
  early usage, and the whole bridge is one `wrangler deploy`.
- The E2E encryption model is unchanged: the server stores ciphertext +
  a revision counter and plaintext host metadata (labels, addresses,
  usernames, tags, notes, settings). It cannot read sealed passwords or keys.
- The same Desktop OAuth client ID is shared by all platforms (PKCE, no
  secret), so users configure nothing.

## Deploying the bridge

See `bridge/README.md`. The RuTerm reference test instance runs at
`https://sync.lagitest.xyz` (test bridge; production domain TBD).
Short version:

```bash
cd bridge
npm install
npx wrangler login
npx wrangler d1 create rterm-sync          # copy database_id into wrangler.toml
npx wrangler secret put JWT_SECRET          # openssl rand -hex 32
npx wrangler secret put GOOGLE_CLIENT_SECRET  # Desktop client secret (never in repo/app)
npm run db:migrate:remote
npm run deploy
```

Custom domain: `routes = [ { pattern = "sync.example.com", custom_domain = true } ]`
— Cloudflare provisions DNS + TLS automatically. A plain
`{ pattern = "…/*", zone_name = "…" }` only routes edge traffic and creates **no**
DNS record (the hostname stays NXDOMAIN).

Then publish your OAuth consent screen to production and point its homepage /
privacy URLs at the bridge (`https://<domain>/`, `https://<domain>/privacy`).
Scope is `openid email profile` only.

## Pointing the app at the bridge

The app resolves the bridge URL in this order:

1. `bridge_url` in `~/Library/Application Support/app.rterm/settings.json`
   (self-hosted / local dev, e.g. `http://127.0.0.1:8787` for `npm run dev`);
2. `RTERM_BRIDGE_URL` baked in at build time (`RTERM_BRIDGE_URL=https://sync.example.com npm run tauri build`).

Official release builds will set `RTERM_BRIDGE_URL` so end users get zero
configuration — there is deliberately no UI field for it.

If neither is set, sync is unavailable and the app shows a clear error; the
local vault keeps working unchanged.

## Keychain keys

| Key | Contents |
| --- | --- |
| `sync-refresh` | RuTerm bridge refresh token (rotated on every use; reuse is rejected) |
| `account` | JSON `{ email, name }` for the signed-in account (display only) |
| `vault-key` | Local vault key cache (never synced) |

The legacy `oauth-refresh` (Google Drive, pre-bridge) is deleted on sign-in.

## Conflict behaviour

`PUT /v1/vault` with a stale `base_revision` returns `409` with the server's
current blob. The app re-pulls, merges (LWW + tombstones) and retries, up to 4
times — same semantics as the old Drive sync.
