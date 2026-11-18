# A fresh coat: IBM Plex, ligatures & a calmer homepage

The whole UI switched from Manrope to **IBM Plex Sans** (with **IBM Plex Mono** for
paths and fingerprints) — it matches the rust ember identity better and reads
warmer at small sizes.

- Every `font-size` in the app went up ~1.5px. Body text is 17px now.
- Terminal keeps **JetBrains Mono Nerd Font**, upgraded to Medium weight with
  programming **ligatures** (`->`, `=>`, `!=` become single glyphs) — the same
  signature look JetBrains IDEs give your editor.
- Half-block terminal art (opencode, htop and friends) now tiles seamlessly —
  line-height 1, zero letter-spacing.
- The empty "No session open" screen was redesigned: an always-on-brand terminal
  card showing vault status and your hosts, one-click connect, and quick links to
  About / Terms / Privacy / Contact / GitHub — all opening as in-app tabs.
