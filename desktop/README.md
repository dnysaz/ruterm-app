# RuTerm desktop app

The desktop app is a Tauri v2 + Svelte 5 shell around the Rust engine in
`../rust`. See the [root README](../README.md) for the full picture.

```bash
npm install
npm run tauri dev      # dev mode with hot reload
npm run tauri build    # produce the macOS app bundle / installer
npm run check          # svelte-check + TypeScript
npm run build          # production frontend only
```

Release notes for the app live in [`../news`](../news/README.md) — plain Markdown,
pulled straight from the repository into the in-app **What's New** tab.
