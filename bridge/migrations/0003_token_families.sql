-- Refresh-token families + device revocation (F-09/F-22).
-- Reuse of a rotated token revokes the whole family and its device.
ALTER TABLE refresh_tokens ADD COLUMN family_id TEXT;
ALTER TABLE refresh_tokens ADD COLUMN replaced_by TEXT;
ALTER TABLE refresh_tokens ADD COLUMN revoked_reason TEXT;
UPDATE refresh_tokens SET family_id = id WHERE family_id IS NULL;
CREATE INDEX refresh_tokens_family ON refresh_tokens(family_id);

ALTER TABLE devices ADD COLUMN revoked_at TEXT;
