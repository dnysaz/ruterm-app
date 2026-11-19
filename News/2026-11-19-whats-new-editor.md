# A real code editor, image previews, and files that hide

The SFTP panel grew up: files open in Monaco, images open as images, and the
browser finally respects dotfiles.

- **Monaco everywhere** — opening a file from SFTP now uses the same editor
  engine as VS Code: syntax highlighting for HTML, CSS, JS/TS, PHP, Python, Go,
  Rust, JSON, YAML, SQL, shell and more, plus a minimap, multiple cursors
  (Option-click or ⌘D), bracket-pair guides, find & replace and folding. The
  editor follows your Dark/Light theme, and it is loaded lazily, so app startup
  stays as fast as before.
- **Images open as images** — PNG, JPEG, GIF, WebP, AVIF, BMP, ICO and SVG are
  previewed in the tab with a transparency checkerboard instead of the old
  "binary file" dead end. Previews go up to 12 MB; anything larger still offers
  Download.
- **Save works on stubborn servers** — some SFTP servers refuse to rename a
  temp file over an existing one. Saving now falls back to overwriting the file
  in place (and, if the file itself is read-only, replacing it while keeping its
  permission bits), so a ⌘S no longer ends in "could not replace the file".
- **Hidden files, hidden by default** — the SFTP toolbar has an eye toggle.
  Dotfiles stay out of the way until you ask for them; if a folder contains only
  hidden items, the empty state offers a one-click "Show hidden files", and the
  status bar tells you how many are hidden.
