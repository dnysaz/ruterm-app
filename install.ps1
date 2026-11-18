# RuTerm installer for Windows — downloads the latest GitHub Release and installs it.
#
#   irm https://raw.githubusercontent.com/dnysaz/ruterm-app/main/install.ps1 | iex
#
# What it does:
#   1. Queries the GitHub API for the latest release of dnysaz/ruterm-app.
#   2. Downloads the NSIS *-setup.exe installer (silent mode).
#   3. Verifies it against SHA256SUMS when the release publishes one.
#
# It NEVER touches your vault, SSH keys or settings: it only runs the app installer.
# Uninstall: Settings → Apps → RuTerm, or uninstall.exe inside the install dir.

$ErrorActionPreference = 'Stop'

$Repo = 'dnysaz/ruterm-app'
$Api  = "https://api.github.com/repos/$Repo"

Write-Host '==> RuTerm installer — Windows/x64' -ForegroundColor Cyan

$release = Invoke-RestMethod -Uri "$Api/releases/latest" -Headers @{ 'User-Agent' = 'ruterm-installer' }

$asset = $release.assets | Where-Object { $_.name -like '*-setup.exe' } | Select-Object -First 1
if (-not $asset) {
  Write-Host 'No NSIS installer found in the latest release. Assets available:' -ForegroundColor Red
  $release.assets | ForEach-Object { Write-Host "  - $($_.name)" }
  exit 1
}

$tmp = Join-Path $env:TEMP ("ruterm-" + [guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

$exe  = Join-Path $tmp $asset.name
$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS' } | Select-Object -First 1

Write-Host "==> Downloading $($asset.name) ($($release.tag_name)) …" -ForegroundColor Cyan
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $exe -UseBasicParsing

if ($sums) {
  Write-Host '==> Verifying SHA256 checksum …' -ForegroundColor Cyan
  $sumsFile = Join-Path $tmp 'SHA256SUMS'
  Invoke-WebRequest -Uri $sums.browser_download_url -OutFile $sumsFile -UseBasicParsing
  $line = Get-Content $sumsFile | Where-Object { $_ -match [regex]::Escape($asset.name) } | Select-Object -First 1
  if (-not $line) { throw 'asset not listed in SHA256SUMS — refusing to install' }
  $expected = ($line -split '\s+')[0].ToLower()
  $actual   = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
  if ($expected -ne $actual) { throw 'checksum mismatch — refusing to install' }
  Write-Host '==> Checksum OK' -ForegroundColor Green
} else {
  Write-Warning 'SHA256SUMS not found — installing without checksum verification'
}

Write-Host '==> Running installer …' -ForegroundColor Cyan
Start-Process -FilePath $exe -ArgumentList '/S' -Wait
Remove-Item -Recurse -Force $tmp

Write-Host ''
Write-Host '==> Done. SmartScreen may warn on first launch: More info → Run anyway (unsigned build).' -ForegroundColor Green
