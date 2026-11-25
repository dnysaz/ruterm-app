# RuTerm v0.1.2 installers

Use `v0.1.2` — this fixes the Windows **local terminal** error:

```text
spawn: CreateProcessW "/bin/zsh -l" ... failed (os error 3)
```

Windows now launches PowerShell 7 / Windows PowerShell / `cmd.exe` instead of
`/bin/zsh`, and inherits the Windows environment variables those shells need.

Builds are still unsigned. Verify checksums before running.

## Windows

- `RuTerm_0.1.2_x64-setup.exe` — NSIS installer (recommended)
- `RuTerm_0.1.2_x64_en-US.msi` — MSI installer

Uninstall `v0.1.0` / `v0.1.1` first, then install this one. Open **Local
terminal** and confirm you get a PowerShell prompt.

## Linux

```bash
sudo apt install ./RuTerm_0.1.2_amd64.deb
# or
sudo dnf install ./RuTerm-0.1.2-1.x86_64.rpm
# or
chmod +x RuTerm_0.1.2_amd64.AppImage && ./RuTerm_0.1.2_amd64.AppImage
```

## macOS Apple Silicon

Open `RuTerm_0.1.2_aarch64.dmg` and drag RuTerm into Applications. If
Gatekeeper blocks it, right-click → **Open** → **Open**, or after copying:

```bash
xattr -dr com.apple.quarantine /Applications/RuTerm.app
```

## Verify

```bash
cd release-assets/v0.1.2
shasum -a 256 -c SHA256SUMS.all    # macOS
sha256sum -c SHA256SUMS.all        # Linux / Git Bash
```

`SHA256SUMS.all` comes from the GitHub release asset digests and covers every
file here. `SHA256SUMS` is the original CI-generated checksum file.
