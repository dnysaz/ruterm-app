# Google OAuth — superseded

The Google Drive sync described in this document was replaced by the RuTerm
**sync bridge** (Cloudflare Worker + D1). See **[SYNC.md](./SYNC.md)** for the
current architecture and deployment steps.

What is still accurate from the old design: one Desktop-type OAuth client for
all platforms, PKCE through the system browser, loopback redirect, and no
client secret in the app; the bridge holds the Desktop client secret
Google requires at the token endpoint.
