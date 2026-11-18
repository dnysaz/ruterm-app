# RuTerm bridge

Serverless sync bridge for RuTerm — Cloudflare Worker + D1. The desktop app stays
fully functional **offline and locally** without it; the bridge only adds cross-device
sync. The server stores **ciphertext only** (end-to-end encrypted vaults), hashed
refresh tokens, and account/device metadata.

## Endpoints

| Method | Path | Purpose |
|---|---|---|
| GET | `/` | Landing page |
| GET | `/privacy` | Privacy policy |
| GET | `/health` | Liveness |
| POST | `/v1/auth/google` | Exchange loopback+PKCE auth code (client holds no secret) → RuTerm token pair |
| POST | `/v1/auth/refresh` | Rotate refresh token → new pair (reuse = 401) |
| POST | `/v1/auth/logout` | Revoke the presented refresh token |
| GET | `/v1/me` | Account + vault revision |
| GET | `/v1/vault` | `{ revision, blob }` (ciphertext) |
| PUT | `/v1/vault` | `{ base_revision, blob }` → 409 with current blob on conflict |
| GET | `/v1/devices` | List registered devices |
| DELETE | `/v1/devices/:id` | Remove a device + its refresh tokens |
| DELETE | `/v1/account` | Erase account + all synced data (cascade) |

## Setup (one time)

```bash
cd bridge
npm install
npx wrangler login
npx wrangler d1 create rterm-sync          # copy the returned database_id
# edit wrangler.toml -> [[d1_databases]] database_id, and [vars] domain
npx wrangler secret put JWT_SECRET          # e.g. openssl rand -hex 32
npm run db:migrate:remote                   # create tables in D1
npm run deploy
```

Point your domain at the Worker. The proven route is the custom-domain one in
`wrangler.toml`:

```toml
routes = [ { pattern = "sync.yourdomain.com", custom_domain = true } ]
```

`wrangler deploy` then provisions DNS + TLS automatically (verified live).
A plain `{ pattern = "…/*", zone_name = "…" }` is only a route — it creates
**no** DNS record. In Google Cloud Console, your OAuth **Desktop app**
client id stays public and is already baked into `[vars]`. Google's token
endpoint still demands the Desktop client secret in the code exchange (it is
not treated as confidential for installed apps — PKCE still applies). The
secret therefore lives only in the Worker secret store:

```bash
npx wrangler secret put GOOGLE_CLIENT_SECRET   # Desktop client secret from the JSON you downloaded
```

It is never in the app and never in this repo. Publish the consent screen to
production and set the homepage/privacy URLs to
`https://sync.yourdomain.com` and `https://sync.yourdomain.com/privacy`.

## Dev

```bash
cp .dev.vars.example .dev.vars   # JWT_SECRET + GOOGLE_CLIENT_SECRET for local dev only
npm run db:migrate               # local D1
npm run dev                      # http://127.0.0.1:8787
npm run typecheck
```

`wrangler dev --local` runs Workers runtime locally; local D1 lives in `.wrangler/state`.
Never commit `.dev.vars` or `.wrangler/` (gitignored at repo root).
