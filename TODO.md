# TODO — Penutupan ekor R-02/R-03 (raw SFTP streaming)

> **Tujuan:** hilangkan dua keterbatasan terakhir yang masih tercatat di `check_2.md §6`:
> - **R-03 ekor:** `russh-sftp 3.0.1` high-level `read_dir()` meng-buffer seluruh listing ke
>   memori sebelum batas 10.000 diterapkan → risiko OOM dari server yang memihak jahat.
> - **R-02 ekor:** `canonicalize()` masih dikendalikan server SFTP.
>
> **Hasil evaluasi:** R-02 ternyata bisa ditutup di jalur listing — `canonicalize()` dihapus,
> path dipakai apa adanya dan entri dibangun lokal. R-03 ditutup dengan `READDIR` streaming.
> Server tetap otoritas filesystem-nya, tetapi path yang dikembalikannya tidak dipercaya.
>
> **Aturan kerja:** semua perubahan **uncommitted** sampai todo terakhir selesai + diverifikasi.
> Jangan commit/push di tengah jalan. Coret `[x]` hanya setelah bukti verifikasi ada.

---

## 1. Riset API low-level `russh-sftp 3.0.1`

- [x] 1.1 Cek apakah `SftpSession` mengekspos raw send/receive packet, atau ada tipe
      `RawSftpSession` / `into_inner()` / `SftpSession::from_stream()` yang bisa dipakai
      tanpa mengganti sesi SSH yang sudah ada.
      → `SftpSession.session` private, tanpa accessor. `RawSftpSession` public & punya
      `opendir`/`readdir`/`close` per paket, tapi harus dibuat dari stream channel sendiri.
- [x] 1.2 Pastikan sesi hasil `sftp_connect()` bisa dipakai untuk perintah high-level
      (read/write/stat) **dan** raw READDIR secara bergantian tanpa konflik request-id.
      Kalau tidak bisa, tentukan apakah perlu session SFTP kedua (channel baru).
      → Tidak bisa bergantian di satu `SftpSession`. Keputusan: **Opsi B** = channel SFTP
      kedua pada `Handle` SSH yang sama, memakai `RawSftpSession` publik. `russh-sftp`
      terbaru tetap 3.0.1; tidak ada upgrade yang mengekspos raw accessor.
- [x] 1.3 Catat format paket yang dipakai: `SSH_FXP_OPENDIR` (11), `SSH_FXP_READDIR` (12),
      `SSH_FXP_CLOSE` (4), `SSH_FXP_NAME` (104), `SSH_FXP_STATUS` (101), `SSH_FX_EOF` (1).
      Format ATTRS: flags + size/uidgid/perm/acmodtime/extended. Kita hanya butuh `filename`
      + `permissions` (S_IFDIR / S_IFLNK).
      → Paket tidak perlu dikodekan manual: `RawSftpSession` menangani framing, request-id,
      timeout, dekode `Name.files`/`Attrs`, dan memetakan EOF ke `Error::Status(Eof)`.

## 2. Implementasi raw READDIR streaming

- [x] 2.1 Pakai API publik `RawSftpSession::opendir/readdir/close` (bukan encoder manual).
      Framing, request-id, timeout, dan dekode ATTRS ditangani `russh-sftp 3.0.1`.
      Bukti: `rust/crates/rterm-core/src/sftp.rs` `RawDirIter` + `open_raw_sftp()`.
- [x] 2.2 `raw_list_dir(raw, path)`: OPENDIR → HANDLE → loop READDIR per paket → kumpulkan
      entri hingga `MAX_DIR_ENTRIES` → CLOSE. Stop segera saat entri ke-10.001 terlihat;
      CLOSE tetap dikirim di semua jalur (cap/error/sukses).
- [x] 2.3 EOF (`StatusCode::Eof`) → akhir listing; status lain → `SshError::Request`.
      Bukti: test `raw_list_propagates_non_eof_status`.
- [x] 2.4 Request-id dialokasikan oleh `RawSftpSession` (counter terpisah dari sesi
      high-level, jadi tidak bentrok) dan `set_timeout(30)` dipakai. Paket dengan id
      tidak cocok diabaikan library → timeout, bukan hang.
      Bukti: test `raw_list_times_out_when_server_never_replies` dan
      `raw_list_ignores_mismatched_request_id_and_times_out`.
- [x] 2.5 Validasi nama (`.`/`..`/`/`/`\\`/NUL ditolak) dipertahankan untuk listing
      **dan** recursive delete; `entry.path` dibangun lokal dari path yang diminta.
      Bukti: `valid_filename()`, `entry_from_file()`, test `raw_list_rejects_server_supplied_traversal_name`.

## 3. Integrasi

- [x] 3.1 `list_dir()` sekarang memanggil `raw_list_dir(&sftp.raw, path)`; tidak ada
      `read_dir()` / `canonicalize()` lagi di jalur listing.
- [x] 3.2 Recursive delete memakai `RawDirIter` langsung dan `charge_delete()` dipanggil
      **per entri di wire**, sebelum entry diproses — bukan setelah seluruh listing
      di-buffer. Nama child juga divalidasi (sebelumnya tidak).
- [x] 3.3 `SftpDir.path` = `path.to_string()` (path yang diminta), dibuktikan test
      `raw_list_decodes_attrs_sorts_and_keeps_requested_path`.

## 4. Evaluasi ulang R-02 ekor

- [x] 4.1 `canonicalize()` **dihilangkan dari `list_dir()`**. Path yang diminta langsung
      dikirim ke OPENDIR dan dikembalikan apa adanya di `SftpDir.path`.
- [x] 4.2 Satu-satunya `canonicalize()` yang tersisa ada di `home_dir()` (best-effort,
      fallback `/`) — itu hanya menentukan direktori awal panel, bukan path entri
      atau path tulis. Semua `SftpEntry.path` dan path delete dibangun lokal dari input
      pengguna. Status R-02 ekor: **tertutup di jalur listing**, bukan lagi "inheren"
      yang hanya diduga. Bukti: komentar di `list_dir()` + test traversal di atas.

## 5. Test

- [x] 5.1 8 test baru di `sftp::tests`: round-trip framing STATUS/HANDLE, dekode
      NAME+ATTRS (size/mtime/perm), sorting, EOF vs error status, traversal rejection.
- [x] 5.2 `accept_entry_never_retains_entry_past_cap` + `raw_list_stops_at_entry_cap`
      (mock server mengirim 10.001 entri; client berhenti di entri ke-10.001).
- [x] 5.3 `raw_list_times_out_when_server_never_replies` +
      `raw_list_ignores_mismatched_request_id_and_times_out` → error, tidak hang.
- [x] 5.4 `cd rust && cargo test --workspace` ✅ 24 passed, 0 failed.
- [x] 5.5 `cd desktop/src-tauri && cargo test` ✅ 3 passed, 0 failed.

## 6. Verifikasi lintas-stack

- [x] 6.1 `cd desktop && npm run check` ✅ 0 error, 5 warning lama (NoteEditor).
- [x] 6.2 `cd desktop && npm run build` ✅ adapter-static, rangkuman chunk besar (info).
- [x] 6.3 `cd bridge && npm run typecheck` ✅ + `npx wrangler deploy --dry-run` ✅ (59.45 KiB).
- [x] 6.4 `npm audit` ✅ 0/0 (desktop prod + full, bridge).

## 7. Dokumentasi

- [x] 7.1 `check_2.md`: §5b R-02/R-03 → DITUTUP, §5c ditambah (bukti file/perubahan +
      verifikasi), §6 ekor diubah dari "inheren" menjadi "sudah ditutup".
- [x] 7.2 `check.md` cross-reference diperbarui ke §5c.
- [x] 7.3 `News/2026-11-20-sftp-streaming.md` ditambah; `node News/build-index.mjs` ✅
      → 13 entri (index + `desktop/static/news/`).

## 8. Finalisasi

- [x] 8.1 `git status`: hanya file kode + `TODO.md` + News yang berubah;
      `check.md`/`check_2.md` tetap ter-ignore (`git ls-files` kosong).
- [ ] 8.2 Commit + push **menunggu otorisasi eksplisit user** — tidak dilakukan otomatis.
- [x] 8.3 Laporkan hasil ke user (ringkasan + status uncommitted).

---

## Log pengerjaan

| Tanggal | Langkah | Catatan |
| --- | --- | --- |
| 2026-11-20 | Todo dibuat | Belum ada kode yang diubah untuk task ini. |
| 2026-11-20 | 1.1–1.3 | Riset selesai: `SftpSession` tidak punya raw accessor; `RawSftpSession` publik dipakai pada channel SFTP kedua di koneksi SSH yang sama. Tidak ada patch/vendor dependency. |
| 2026-11-20 | 2–3 | Implementasi `SftpConnection` (high-level + raw + SSH handle, `Deref` ke `SftpSession`), `RawDirIter`, `raw_list_dir`, validasi nama di listing & delete. `state.rs`/`commands.rs` disesuaikan. `cargo check` Rust workspace + desktop ✅. |
| 2026-11-20 | 4 | `canonicalize()` dihapus dari `list_dir()`; tersisa hanya di `home_dir()` untuk direktori awal. Path entri tetap lokal. |
| 2026-11-20 | 5 | 8 unit test baru; `cargo test --workspace` ✅ 24/24; `desktop/src-tauri cargo test` ✅ 3/3. |
| 2026-11-20 | 6 | `npm run check` ✅ 0 error/5 warning; `npm run build` ✅; bridge typecheck + dry-run ✅; `npm audit` ✅ 0/0. |
| 2026-11-20 | 7 | `check_2.md` §5b/§5c/§6 + `check.md` cross-ref diperbarui; News baru + index 13 entri. |
| 2026-11-20 | 8 | `git status` diverifikasi. Commit/push **tidak dilakukan** — menunggu izin eksplisit. |
