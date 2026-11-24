#!/bin/sh
# RuTerm installer — downloads the latest GitHub Release and installs it.
#
#   curl -fsSL https://raw.githubusercontent.com/dnysaz/ruterm-app/main/install.sh | sh
#
# What it does:
#   1. Detects OS (macOS / Linux) and architecture (arm64 / x86_64).
#   2. Queries the GitHub API for the latest release of dnysaz/ruterm-app.
#   3. Downloads the matching asset and requires a matching entry in the
#      published SHA256SUMS. Missing or mismatched checksums abort the install.
#   4. Installs:
#        macOS → RuTerm.app copied into /Applications (fallback: ~/Applications)
#        Linux → the .deb via apt (root/sudo), else the AppImage into ~/.local/bin
#
# Options (pass as flags or environment):
#   --dry-run   detect platform, print what would be downloaded/installed, exit
#   --yes       skip the confirmation prompt (CI / non-interactive use)
#   --version v0.1.0  install a specific release instead of latest
#
# It NEVER touches your vault, SSH keys or settings: it only places the app.
# Uninstall: macOS — drag /Applications/RuTerm.app to the Trash.
#            Linux — sudo apt remove ruterm  (or rm ~/.local/bin/ruterm).

set -eu

REPO="dnysaz/ruterm-app"
API="https://api.github.com/repos/${REPO}"
DRY_RUN=0
ASSUME_YES=0
VERSION=""

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --yes|-y)  ASSUME_YES=1 ;;
    --version) VERSION="${2:-}"; shift ;;
    -h|--help)
      sed -n '2,30p' "$0"
      exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

say()  { printf '%s\n' "$*"; }
info() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }

need curl
need uname

OS="$(uname -s)"
ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
  arm64|aarch64) IS_ARM=1; IS_X64=0 ;;
  x86_64|amd64)  IS_ARM=0; IS_X64=1 ;;
  *) die "unsupported architecture: $ARCH_RAW" ;;
esac
case "$OS" in
  Darwin|Linux) ;;
  *) die "unsupported OS: $OS (macOS and Linux only; on Windows use install.ps1)" ;;
esac

info "RuTerm installer — ${OS}/${ARCH_RAW}"

# ── Resolve the release to install ────────────────────────────────────────────
if [ -n "$VERSION" ]; then
  info "Fetching release ${VERSION} …"
  RELEASE_JSON="$(curl -fsSL "${API}/releases/tags/${VERSION}")" \
    || die "release ${VERSION} not found"
else
  info "Fetching latest release …"
  RELEASE_JSON="$(curl -fsSL "${API}/releases/latest")" \
    || die "no releases found yet — see https://github.com/${REPO}/releases"
fi

TAG="$(printf '%s' "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4)"
DL_URLS="$(printf '%s' "$RELEASE_JSON" | grep -o '"browser_download_url": *"[^"]*"' | cut -d'"' -f4)"
[ -n "$TAG" ] && [ -n "$DL_URLS" ] || die "could not parse release metadata"

info "Release: ${TAG}"

# ── Pick the asset for this platform ─────────────────────────────────────────
pick_asset() {
  # Tauri names artifacts with the Rust triple arch: aarch64 / x86_64 / amd64.
  if [ "$OS" = "Darwin" ]; then
    if [ "$IS_ARM" = 1 ]; then
      A="$(printf '%s\n' "$DL_URLS" | grep -E '\.dmg$' | grep -E '(aarch64|arm64)\.dmg$' | head -1 || true)"
    else
      A="$(printf '%s\n' "$DL_URLS" | grep -E '\.dmg$' | grep -E '(x86_64|x64|amd64)\.dmg$' | head -1 || true)"
    fi
    [ -n "$A" ] || A="$(printf '%s\n' "$DL_URLS" | grep -E '\.dmg$' | head -1 || true)"
    printf '%s\n' "$A"
    return
  fi
  # Linux: prefer .deb for the exact arch, else any AppImage
  if [ "$IS_X64" = 1 ]; then
    A="$(printf '%s\n' "$DL_URLS" | grep -E '\.deb$' | grep -E '(amd64|x86_64)\.deb$' | head -1 || true)"
  else
    A="$(printf '%s\n' "$DL_URLS" | grep -E '\.deb$' | grep -E '(arm64|aarch64)\.deb$' | head -1 || true)"
  fi
  [ -n "$A" ] || A="$(printf '%s\n' "$DL_URLS" | grep -E '\.AppImage$' | head -1 || true)"
  printf '%s\n' "$A"
}

ASSET_URL="$(pick_asset)"
[ -n "$ASSET_URL" ] || die "no installer for ${OS}/${ARCH} in ${TAG} yet.
Available assets:
$(printf '%s\n' "$DL_URLS")"

ASSET_NAME="${ASSET_URL##*/}"
info "Asset: ${ASSET_NAME}"

# ── Confirm ───────────────────────────────────────────────────────────────────
if [ "$ASSUME_YES" != 1 ] && [ "$DRY_RUN" != 1 ]; then
  printf 'Install RuTerm %s to this machine? [y/N] ' "$TAG"
  read -r REPLY || REPLY=""
  case "$REPLY" in
    y|Y|yes|YES) ;;
    *) say "Aborted."; exit 0 ;;
  esac
fi

if [ "$DRY_RUN" = 1 ]; then
  say ""
  say "DRY RUN — would download: ${ASSET_URL}"
  say "DRY RUN — would require a matching SHA256SUMS entry, then install."
  exit 0
fi

# ── Download + verify ─────────────────────────────────────────────────────────
TMPDIR_DL="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_DL"' EXIT HUP INT TERM

info "Downloading ${ASSET_NAME} …"
curl -fL# -o "${TMPDIR_DL}/${ASSET_NAME}" "$ASSET_URL"

if SUMS="$(curl -fsSL "${API}/releases/tags/${TAG}" | grep -o '"browser_download_url": *"[^"]*SHA256SUMS[^"]*"' | cut -d'"' -f4 | head -1)" && [ -n "$SUMS" ]; then
  info "Verifying SHA256 checksum …"
  curl -fsSL -o "${TMPDIR_DL}/SHA256SUMS" "$SUMS"
  EXPECTED="$(grep " ${ASSET_NAME}\$" "${TMPDIR_DL}/SHA256SUMS" | awk '{print $1}')"
  [ -n "$EXPECTED" ] || die "asset not listed in SHA256SUMS — refusing to install"
  if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "${TMPDIR_DL}/${ASSET_NAME}" | awk '{print $1}')"
  else
    ACTUAL="$(shasum -a 256 "${TMPDIR_DL}/${ASSET_NAME}" | awk '{print $1}')"
  fi
  [ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch — refusing to install"
  info "Checksum OK"
else
  die "SHA256SUMS not found for ${TAG} — refusing to install an unverified asset"
fi

# ── Install ───────────────────────────────────────────────────────────────────
case "$OS" in
  Darwin)
    need hdiutil
    need cp
    info "Mounting disk image …"
    MOUNT_POINT="$(hdiutil attach -nobrowse -readonly "${TMPDIR_DL}/${ASSET_NAME}" | tail -1 | awk -F'\t' '{print $NF}')"
    APP_PATH="$(find "$MOUNT_POINT" -maxdepth 2 -name '*.app' -type d | head -1)"
    [ -n "$APP_PATH" ] || { hdiutil detach "$MOUNT_POINT" >/dev/null; die "no .app found in the disk image"; }
    DEST="/Applications"
    [ -w "$DEST" ] || DEST="$HOME/Applications"
    mkdir -p "$DEST"
    info "Copying $(basename "$APP_PATH") → ${DEST} …"
    rm -rf "${DEST}/$(basename "$APP_PATH")"
    cp -R "$APP_PATH" "$DEST/"
    hdiutil detach "$MOUNT_POINT" >/dev/null
    # Do not strip the quarantine attribute: Gatekeeper stays a real barrier.
    info "Installed to ${DEST}/$(basename "$APP_PATH")"
    if [ -z "$(codesign -dv "${DEST}/$(basename "$APP_PATH")" 2>&1 | grep -i '^Signature=' | grep -iv 'invalid')" ]; then
      warn "RuTerm is unsigned — verify SHA256SUMS and decide yourself whether to allow it in Gatekeeper"
    fi
    ;;
  Linux)
    case "$ASSET_NAME" in
      *.deb)
        need sudo
        info "Installing package …"
        sudo apt-get install -y "${TMPDIR_DL}/${ASSET_NAME}"
        info "Installed. Launch from your app menu or run: ruterm"
        ;;
      *.AppImage)
        DEST="$HOME/.local/bin"
        mkdir -p "$DEST"
        install -m 0755 "${TMPDIR_DL}/${ASSET_NAME}" "${DEST}/ruterm"
        info "Installed to ${DEST}/ruterm — make sure ~/.local/bin is on your PATH"
        ;;
    esac
    ;;
esac

say ""
info "Done — RuTerm ${TAG} installed."
