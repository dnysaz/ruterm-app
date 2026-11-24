# Your old key still works — and a calmer home screen

Older vaults now open with the correct encryption context, so your existing key is accepted again and re-sealed automatically.

A hardening change earlier this week made RuTerm read older vaults with the
wrong encryption context, so a correct key could be reported as "Wrong
passphrase". That regression is fixed: a compatibility fallback reads the old
ciphertext and the next unlock or sync quietly re-seals it in the current
format. Nothing about your hosts, identities or notes is discarded.

The sign-in screens now use a proper Google button with the four-colour logo and
"Continue with Google". Wherever an account is shown, "Not [email]? Change
account" signs out and reopens Google's account chooser; Settings and the vault
sidebar have a Change account action too. If a vault really cannot be opened,
the Unlock screen has a last-resort reset that asks for a typed `RESET`
confirmation and warns that all local hosts, identities and notes are erased.

Home picked up a few quality-of-life changes. Settings → Appearance can hide
Quick connect and replace it with a clock and the RuTerm | SSH Client wordmark.
Borders are now a soft blue, AI agent launchers moved from the floating dock
into the sidebar (grey until hovered), and the clock card is lighter and easier
to read. Double-clicking an image in the SFTP panel now opens a right-hand
preview with format, size, dimensions, modified time, permissions and full path
plus a close button; text files still open in tabs. File rows use larger icons:
solid yellow folders and extension badges for files.
