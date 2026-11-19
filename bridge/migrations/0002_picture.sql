-- Add the Google profile photo URL to users.
-- Apply with:
--   wrangler d1 migrations apply rterm-sync --local   (dev)
--   wrangler d1 migrations apply rterm-sync --remote  (production)
ALTER TABLE users ADD COLUMN picture TEXT;
