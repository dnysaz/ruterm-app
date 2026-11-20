-- rterm-sync D1 schema. Apply with:
--   wrangler d1 migrations apply rterm-sync --local   (dev)
--   wrangler d1 migrations apply rterm-sync --remote  (production)

CREATE TABLE IF NOT EXISTS users (
  id          TEXT PRIMARY KEY,
  google_sub  TEXT NOT NULL UNIQUE,
  email       TEXT NOT NULL,
  name        TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS devices (
  id          TEXT PRIMARY KEY,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  name        TEXT NOT NULL DEFAULT 'Unknown device',
  platform    TEXT NOT NULL DEFAULT 'unknown',
  app_version TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  last_seen_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS devices_user ON devices(user_id);

CREATE TABLE IF NOT EXISTS refresh_tokens (
  id          TEXT PRIMARY KEY,
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  device_id   TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
  token_hash  TEXT NOT NULL UNIQUE,
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  expires_at  TEXT NOT NULL,
  revoked_at  TEXT
);
CREATE INDEX IF NOT EXISTS refresh_tokens_lookup ON refresh_tokens(token_hash);
CREATE INDEX IF NOT EXISTS refresh_tokens_user ON refresh_tokens(user_id);

-- Secret fields are sealed on the client before upload. Host metadata in the
-- vault JSON stays in plain text and is readable by the operator.
CREATE TABLE IF NOT EXISTS vaults (
  user_id     TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  revision    INTEGER NOT NULL DEFAULT 0,
  blob        TEXT NOT NULL DEFAULT '',
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
