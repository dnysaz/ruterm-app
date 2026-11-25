# RuTerm installers

**Latest: [v0.1.2](v0.1.2/)** — fixes the Windows startup crash and the local
terminal shell error. Use that folder.

The files in this folder are the archived `v0.1.0` installers, copied from the
GitHub release for tag `v0.1.0` (commit `70f4c7e`) so they can still be
downloaded directly from this repository.

Builds are **unsigned**. Verify the SHA-256 checksums before running anything.

## Verify

macOS:

```bash
cd release-assets
shasum -a 256 -c SHA256SUMS.all
```

Linux:

```bash
cd release-assets
sha256sum -c SHA256SUMS.all
```

`SHA256SUMS.all` is generated from the GitHub release asset digests and covers
all files here. `SHA256SUMS` is the original checksum file produced by CI and
covers the five primary installers.

## Windows

- `RuTerm_0.1.0_x64-setup.exe` — NSIS installer (recommended)
- `RuTerm_0.1.0_x64_en-US.msi` — MSI installer

Because the build is unsigned, SmartScreen may warn. Verify the checksum first,
then choose **More info → Run anyway** if you trust this build.

## Linux

Debian / Ubuntu:

```bash
sudo apt install ./RuTerm_0.1.0_amd64.deb
```

Fedora / RHEL:

```bash
sudo dnf install ./RuTerm-0.1.0-1.x86_64.rpm
```

AppImage (no install required):

```bash
chmod +x RuTerm_0.1.0_amd64.AppImage
./RuTerm_0.1.0_amd64.AppImage
```

## macOS Apple Silicon

- `RuTerm_0.1.0_aarch64.dmg` — open, drag RuTerm into Applications
- `RuTerm_aarch64.app.tar.gz` — manual `.app` archive

Because the build is unsigned, Gatekeeper may block first launch. After copying
to `/Applications`, either right-click → **Open** → **Open**, or run:

```bash
xattr -dr com.apple.quarantine /Applications/RuTerm.app
```

Only do this after verifying the checksum above.
