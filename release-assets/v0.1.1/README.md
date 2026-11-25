# RuTerm v0.1.1 installers

This release fixes the Windows startup crash: RuTerm no longer requires the
Unix `HOME` variable, and the MCP bridge token can be overwritten correctly on
Windows. Download `v0.1.1`, not `v0.1.0`.

Builds are still **unsigned**. Verify the SHA-256 checksums before running.

## Verify

macOS:

```bash
cd release-assets/v0.1.1
shasum -a 256 -c SHA256SUMS.all
```

Linux / Windows Git Bash:

```bash
cd release-assets/v0.1.1
sha256sum -c SHA256SUMS.all
```

`SHA256SUMS.all` is generated from the GitHub release asset digests and covers
all files here. `SHA256SUMS` is the original checksum file produced by CI.

## Windows

- `RuTerm_0.1.1_x64-setup.exe` — NSIS installer (recommended)
- `RuTerm_0.1.1_x64_en-US.msi` — MSI installer

Uninstall the old `v0.1.0` first if it is still installed, then run the new
installer. SmartScreen may still warn because the build is unsigned.

## Linux

Debian / Ubuntu:

```bash
sudo apt install ./RuTerm_0.1.1_amd64.deb
```

Fedora / RHEL:

```bash
sudo dnf install ./RuTerm-0.1.1-1.x86_64.rpm
```

AppImage:

```bash
chmod +x RuTerm_0.1.1_amd64.AppImage
./RuTerm_0.1.1_amd64.AppImage
```

## macOS Apple Silicon

- `RuTerm_0.1.1_aarch64.dmg` — open, drag RuTerm into Applications
- `RuTerm_aarch64.app.tar.gz` — manual `.app` archive

If Gatekeeper blocks the first launch, right-click → **Open** → **Open**, or:

```bash
xattr -dr com.apple.quarantine /Applications/RuTerm.app
```
