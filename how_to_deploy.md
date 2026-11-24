# Cara Build & Deploy RuTerm (macOS)

![RuTerm icon](desktop/static/app-icon.png)

Ikon aplikasi kanonik ada di `desktop/static/app-icon.png` (salinan dari
`~/Downloads/RuTerm_.png`) dan dipakai juga di README serta halaman About.

Catatan ini dibuat setelah incident **CSS terminal (xterm) dan code editor (Monaco) tidak terload di release bundle**, padahal di `npm run dev`正常. Ikuti persis supaya tidak terulang.

---

## 1. Akar masalah CSS (WAJIB DIPAHAMI)

### Gejala
- Release `.app` terpasang, UI utama (sidebar, tab, settings) normal.
- Terminal tampil "kacau", seperti web tanpa CSS; `htop` / `opencode` tidak berwarna.
- Monaco (editor SFTP) tampil polos, tidak ada syntax highlighting.

### Penyebab
Vite/SvelteKit **memecah CSS menjadi chunk terpisah** untuk modul yang di-import secara lazy:
- `@xterm/xterm/css/xterm.css` → chunk `2.?????.css`
- Monaco CSS → chunk `editor.?????.css` + `vs.?????.css`

Chunk ini **disuntikkan lewat JavaScript saat runtime** (`<link>` ditambahkan dinamis ke `<head>`). Di browser biasa (Chrome/Firefox) ini works. Di **Tauri WKWebView macOS**, suntikan runtime ini **tidak reliably diterapkan** — stylesheet-nya tidak pernah dipakai, sehingga xterm & Monaco render tanpa style.

CSP tidak boleh dimatikan. Gunakan CSP yang mengizinkan `<style>` inline lewat `style-src 'self' 'unsafe-inline'`; itu cukup untuk WKWebView tanpa membuka celah script inline.

### Fix yang dipakai (SUDAH TERBUKTI AMAN)
Dua perubahan, **keduanya wajib ada**:

1. **Inline semua CSS ke `index.html` saat build**
   - File: `desktop/scripts/link-all-css.mjs`
   - Dipanggil dari `desktop/package.json`:
     ```json
     "build": "vite build && node scripts/link-all-css.mjs"
     ```
   - Script ini membaca semua `.css` di `build/_app/immutable/assets/`, lalu menuliskannya sebagai blok `<style data-inline-css="...">` langsung di `<head>` `build/index.html`.
   - Juga me-rewrite `url(...)` relatif (font, codicon) jadi path absolut `/...` supaya tetap ke-load.
   - **Jangan dihapus** dan jangan diubah menjadi `<link>` lagi.

2. **Biarkan CSP aktif di `desktop/src-tauri/tauri.conf.json`**
   ```json
   "security": {
     "csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; ...",
     "devCsp": "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; ..."
   }
   ```
   - `style-src 'unsafe-inline'` membuat inline `<style>` tetap dipakai WKWebView.
   - `script-src 'self'` (tanpa `unsafe-inline` di produksi) menahan script suntikan; jangan kembalikan ke `null`.

> Catatan: `vite.config.js` tidak boleh pakai `build.cssCodeSplit: false` — SvelteKit meng-override opsi itu (warning `vite_config_overridden`), jadi tidak ada efek.

---

## 2. Langkah build release (copy-paste)

Dari root repo:

```bash
cd /Users/ketutdana/Desktop/rterm/desktop

# Pastikan tidak ada app lama yang jalan
osascript -e 'tell application "RuTerm" to quit' 2>&1 || true

# Build frontend + bundle release (≈7-8 menit, LTO)
npm run tauri build
```

Output:
- `desktop/src-tauri/target/release/bundle/macos/RuTerm.app`
- `desktop/src-tauri/target/release/bundle/dmg/RuTerm_0.1.0_x64.dmg`

---

## 3. Install ke `/Applications`

```bash
ps aux | grep '/Applications/RuTerm.app' | grep -v grep | awk '{print $2}' | xargs -r kill
rm -rf /Applications/RuTerm.app
cp -R /Users/ketutdana/Desktop/rterm/desktop/src-tauri/target/release/bundle/macos/RuTerm.app /Applications/RuTerm.app
xattr -d com.apple.quarantine /Applications/RuTerm.app 2>/dev/null || true
git -C /Users/ketutdana/Desktop/rterm checkout -- news/index.json 2>/dev/null || true
open -a /Applications/RuTerm.app
```

`git checkout -- news/index.json` penting karena `npm run build` (prebuild) selalu regenerate `news/index.json` + `desktop/static/news/`. Jangan sampai ikut ter-commit.

---

## 4. Bersihkan cache WebView (jika CSS masih terasa lama)

Lokasi cache WKWebView yang pernah dipakai:
- `~/Library/Caches/rterm-app`
- `~/Library/Caches/app.rterm`
- `~/Library/WebKit/rterm-app`
- `~/Library/WebKit/app.rterm`

Pindahkan (jangan `rm` langsung) ke folder backup di temp:

```bash
backup=/private/var/folders/_f/36tbgdkn1ng4g7tl1wfcbs6r0000gn/T/opencode/rterm-webkit-cache-backup
mkdir -p "$backup"
for d in ~/Library/Caches/rterm-app ~/Library/Caches/app.rterm \
         ~/Library/WebKit/rterm-app ~/Library/WebKit/app.rterm; do
  [ -d "$d" ] && mv "$d" "$backup/$(basename "$d")"
done
```

Data vault **tidak ikut** tersentuh — vault ada di `~/Library/Application Support/app.rterm/` (`vault.json`, `settings.json`).

---

## 5. Verifikasi cepat CSS masuk bundle

```bash
python3 - <<'PY'
import glob, brotli
for f in glob.glob('/Users/ketutdana/Desktop/rterm/desktop/src-tauri/target/release/build/rterm-app-*/out/tauri-codegen-assets/*.html'):
    try:
        txt = brotli.decompress(open(f,'rb').read()).decode('utf-8','replace')
    except Exception:
        continue
    if 'data-inline-css' in txt:
        print('inline blocks:', txt.count('data-inline-css='))
        print('xterm rules  :', txt.count('.xterm{'))
        print('monaco rules :', txt.count('.monaco-editor'))
        break
PY
```

Output aman: `inline blocks: 5`, `xterm rules ≥ 1`, `monaco rules ≥ 500`.

Atau cek langsung HTML hasil build:

```bash
grep -c 'data-inline-css=' /Users/ketutdana/Desktop/rterm/desktop/build/index.html
# harus 5
grep -c 'rel="stylesheet"' /Users/ketutdana/Desktop/rterm/desktop/build/index.html
# harus 0
```

---

## 6. Checklist sebelum deploy

- [ ] `npm run check` → 0 error
- [ ] `npm run tauri build` sukses
- [ ] `build/index.html` punya 5 blok `data-inline-css`, 0 `<link rel="stylesheet">`
- [ ] `tauri.conf.json` → CSP aktif, `style-src` mengizinkan `'unsafe-inline'`, `script-src` tidak
- [ ] `desktop/scripts/link-all-css.mjs` ada dan dipanggil di `package.json`
- [ ] `news/index.json` berisi `sha256` tiap entry dan ikut di-commit saat rilis
- [ ] `.app` dicopy ke `/Applications/RuTerm.app`, quarantine dihapus
- [ ] Buka app, coba `htop` / `opencode` di terminal + buka file SFTP di editor
- [ ] Jangan commit/push tanpa izin eksplisit

---

## 7. Riwayat singkat

- `e34793d` — vault recovery + change account
- `5f56f1b` — About panel terminal-only icon
- `157d6bc` — hapus fitur Data files + fix About reopen
- CSS fix (belum di-commit): `desktop/package.json`, `desktop/scripts/link-all-css.mjs`, `desktop/src-tauri/tauri.conf.json`

---

## 8. AI Agent via embedded MCP bridge

Agent berjalan **di Mac**, VPS tidak perlu install agen apa pun. RuTerm menyalakan bridge loopback `127.0.0.1:<port>/mcp` dengan token random per-launch.

Agen yang terdeteksi: OpenCode, Freebuff, Kilo Code, Claude, Kiro, Command AI (`cmd`).

Konfigurasi MCP ditulis RuTerm ke folder privat (tidak menyentuh config asli agen):

| Agen | File privat |
| --- | --- |
| OpenCode | env `OPENCODE_CONFIG_CONTENT` + `opencode --standalone` |
| Freebuff | `~/.ruterm/agents/freebuff/.agents/mcp.json` |
| Kilo Code | `~/.ruterm/agents/kilo/kilo.json` |
| Claude | `~/.ruterm/agents/claude/mcp.json` + `--strict-mcp-config` |
| Kiro | `~/.ruterm/agents/kiro/.kiro/settings/mcp.json` |
| Command AI | `~/.ruterm/agents/commandai/.mcp.json` |

Memory per-VPS: `~/.ruterm/sessions/<user>-<host>-<port>.md`, diakses lewat `ruterm_read_session` / `ruterm_note_session`.

Untuk perintah lama: `ruterm_start_execute` → `ruterm_read_stream` (chunk real-time) → `ruterm_cancel_stream` jika perlu. `timeout_ms: 0` = tanpa batas; untuk TUI interaktif pakai `ruterm_send_keys` dengan `include_screen: true` + `ruterm_read_screen`. `ruterm_execute_batch` menjalankan beberapa perintah pendek berurutan.

SSH yang putus (misalnya VPS reboot) di-reconnect otomatis pada tab yang sama: muncul overlay "Connecting…" dengan nomor percobaan, lalu kembali live tanpa buka tab baru. Menutup tab secara manual atau mengetik `exit` tidak memicu reconnect. Host-key pin tetap diverifikasi di setiap percobaan.

Verifikasi cepat setelah klik agen di sidebar:

```bash
find ~/.ruterm/agents -type f -maxdepth 4 -print
ls -l ~/.ruterm/sessions
```

Perintah berisiko tetap lewat modal **Deny / Allow / Always allow**. `Always allow` hanya berlaku untuk command persis yang sama selama RuTerm masih jalan.
