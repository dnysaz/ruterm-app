import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const requestedHost = process.env.TAURI_DEV_HOST;
const LOOPBACK_HOSTS = new Set(['127.0.0.1', 'localhost', '::1']);
const LAN_DEV_OPT_IN = process.env.TAURI_ALLOW_LAN_DEV === '1';

// Default to loopback only. Requiring an explicit opt-in keeps the unencrypted
// Vite HMR socket off the LAN when someone exports TAURI_DEV_HOST=0.0.0.0.
if (requestedHost && !LOOPBACK_HOSTS.has(requestedHost) && !LAN_DEV_OPT_IN) {
  throw new Error(
    `TAURI_DEV_HOST=${requestedHost} is not loopback. Set TAURI_ALLOW_LAN_DEV=1 to opt in.`,
  );
}
const host = requestedHost || '127.0.0.1';

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [
    // SvelteKit 3 moved all config into the Vite plugin (no svelte.config.js).
    sveltekit({
      preprocess: vitePreprocess(),
      adapter: adapter({ fallback: "index.html" }),
    }),
  ],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
