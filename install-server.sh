#!/usr/bin/env bash
# install-server.sh — Install or update lanprobe-server on Debian/Ubuntu (headless)
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh | sudo bash
#   or: sudo bash install-server.sh [--version v0.6.10]
#
# What it does:
#   1. Detects the latest release (or uses --version if provided)
#   2. Downloads lanprobe-server_vX.Y.Z_amd64.deb from GitHub Releases
#   3. Installs (or upgrades) via dpkg
#   4. Restarts the service if it was already running

set -euo pipefail

REPO="Benjamin-Chianese/lanprobe"
SERVICE="lanprobe-server"
ARCH="amd64"
INSTALL_VERSION=""

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version|-v)
      INSTALL_VERSION="$2"
      shift 2
      ;;
    --help|-h)
      sed -n '2,8p' "$0" | sed 's/^# //'
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

# ── Root check ────────────────────────────────────────────────────────────────
if [[ $EUID -ne 0 ]]; then
  echo "Error: this script must be run as root (sudo)." >&2
  exit 1
fi

# ── Dependencies ──────────────────────────────────────────────────────────────
for cmd in curl dpkg; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "Error: '$cmd' is required but not installed." >&2
    exit 1
  fi
done

# ── Resolve version ───────────────────────────────────────────────────────────
if [[ -z "$INSTALL_VERSION" ]]; then
  echo "Fetching latest release version..."
  INSTALL_VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | head -1 \
    | sed 's/.*"tag_name": *"\(.*\)".*/\1/')
  if [[ -z "$INSTALL_VERSION" ]]; then
    echo "Error: could not determine latest release. Use --version vX.Y.Z to specify." >&2
    exit 1
  fi
fi

# Strip leading 'v' for the deb filename, keep it for the tag
TAG="${INSTALL_VERSION}"
VERSION_BARE="${INSTALL_VERSION#v}"

DEB_FILE="${SERVICE}_${TAG}_${ARCH}.deb"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/${DEB_FILE}"
TMP_DEB="/tmp/${DEB_FILE}"

# ── Check if already at this version ─────────────────────────────────────────
INSTALLED_VERSION=$(dpkg-query -W -f='${Version}' "${SERVICE}" 2>/dev/null || true)
if [[ "$INSTALLED_VERSION" == "$VERSION_BARE" ]]; then
  echo "lanprobe-server ${INSTALL_VERSION} is already installed."
  echo "To force reinstall, run: sudo dpkg -i <deb>"
  exit 0
fi

# ── Download ──────────────────────────────────────────────────────────────────
echo "Downloading ${DEB_FILE}..."
if ! curl -fL --progress-bar -o "$TMP_DEB" "$DOWNLOAD_URL"; then
  echo "Error: download failed. Check that ${INSTALL_VERSION} exists on GitHub Releases." >&2
  rm -f "$TMP_DEB"
  exit 1
fi

# ── Check if service was running (to restart after install) ───────────────────
SERVICE_WAS_ACTIVE=false
if systemctl is-active --quiet "${SERVICE}" 2>/dev/null; then
  SERVICE_WAS_ACTIVE=true
fi

# ── Install / upgrade ─────────────────────────────────────────────────────────
echo "Installing ${DEB_FILE}..."
dpkg -i "$TMP_DEB" || {
  echo "dpkg reported errors — running apt-get -f install to fix dependencies..."
  apt-get -f install -y
}
rm -f "$TMP_DEB"

# ── Service management ────────────────────────────────────────────────────────
if [[ -d /run/systemd/system ]]; then
  systemctl daemon-reload
  if $SERVICE_WAS_ACTIVE; then
    echo "Restarting ${SERVICE}..."
    systemctl restart "${SERVICE}"
  fi
  systemctl is-active --quiet "${SERVICE}" && STATUS="running" || STATUS="stopped"
  echo ""
  echo "lanprobe-server ${INSTALL_VERSION} installed — service is ${STATUS}."
  echo ""
  echo "  Status : sudo systemctl status ${SERVICE}"
  echo "  Logs   : sudo journalctl -u ${SERVICE} -f"
  echo "  Config : /var/lib/lanprobe/"
  echo ""
  echo "  First-time setup — the initial admin account is protected by a"
  echo "  one-time setup token (regenerated on every start until an admin"
  echo "  exists). Read it, then enter it in the web setup form:"
  echo "    sudo cat /var/lib/lanprobe/setup-token"
else
  echo ""
  echo "lanprobe-server ${INSTALL_VERSION} installed (no systemd detected)."
  echo "Start manually: /usr/bin/lanprobe-server --host 0.0.0.0 --port 8443"
fi
