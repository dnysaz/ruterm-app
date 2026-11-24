# Closing the last gaps

A short follow-up to today's security update. Three remaining items are now
closed.

## Signing out can't reach across devices anymore

Signing out used to revoke a whole session family based on a refresh token,
without checking that it belonged to the device you were signed in as. If a
refresh token from one device was ever paired with an access token from
another, it could log out the wrong device. The check is now tied to the
device, so a sign-out only affects the session it actually belongs to.

## A stricter content policy

The app's Content Security Policy no longer allows inline scripts. Tauri
hashes the few inline bootstrap scripts at build time, so everything still
starts normally while the browser has one less loose end to exploit.

## Up-to-date web framework

The SvelteKit framework underneath the UI was upgraded to the current major
version. That clears the last dependency advisories — the production and
development dependency audits are both clean.

---

*As before, none of this changes how RuTerm looks or behaves day to day.*
