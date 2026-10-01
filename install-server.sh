#!/usr/bin/env bash
# install-server.sh — Install or update lanprobe-server on Debian/Ubuntu (headless)
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Benjamin-Chianese/lanprobe/main/install-server.sh | sudo bash
#   or: sudo bash install-server.sh [--version v0.6.10]
#
# What it does:
#   1. Detects the latest release (or uses --version if provided)
#   2. Downloads lanprobe-server_vX.Y.Z_amd64.deb from GitHub Releases
#   3. VERIFIES its minisign signature against the LanProbe release key
#   4. Installs (or upgrades) via dpkg
#   5. Restarts the service if it was already running
#   6. Enrols the probe if it is not attached to a hub yet
#
# Options:
#   --version vX.Y.Z   install that release instead of the latest
#   --hub <url>        hub URL, for an unattended enrolment
#   --code <code>      enrolment code (hub -> Fleet -> + on a site)
#   --insecure-skip-verify   install WITHOUT checking the signature (don't)

set -euo pipefail

REPO="Benjamin-Chianese/lanprobe"
SERVICE="lanprobe-server"
ARCH="amd64"
INSTALL_VERSION=""
HUB_URL=""
ENROL_CODE=""
SKIP_VERIFY=false
CONFIG_DIR="/var/lib/lanprobe"

# Clé publique de release LanProbe — la MÊME que celle embarquée dans l'updater
# de l'application (`src-tauri/src/updater.rs`, UPDATER_PUBKEY).
#
# 🔴 Sans cette vérification, ce script installait ce que GitHub lui rendait, sur
# la seule foi du TLS. Un dépôt compromis, un miroir hostile ou un proxy
# d'entreprise qui réécrit une réponse passaient sans un mot — et le paquet
# s'installe en root, sur une machine de client.
PUBKEY="RWTXoaoMukXWw4tfkDWUHR5GAgg5hNGy0t7XitCHbAkZhnR2N9wbsKeg"

# ── Argument parsing ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version|-v)
      INSTALL_VERSION="$2"
      shift 2
      ;;
    --hub)
      HUB_URL="$2"
      shift 2
      ;;
    --code)
      ENROL_CODE="$2"
      shift 2
      ;;
    --insecure-skip-verify)
      SKIP_VERIFY=true
      shift
      ;;
    --help|-h)
      sed -n '2,20p' "$0" | sed 's/^# //'
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
DEPS=(curl dpkg)
if ! $SKIP_VERIFY; then
  DEPS+=(minisign base64)
fi
for cmd in "${DEPS[@]}"; do
  if ! command -v "$cmd" &>/dev/null; then
    echo "Error: '$cmd' is required but not installed." >&2
    if [[ "$cmd" == "minisign" ]]; then
      echo "  Install it:  sudo apt-get install -y minisign" >&2
      echo "  It is what proves the package really comes from LanProbe." >&2
    fi
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

# ── Verify the signature BEFORE installing ────────────────────────────────────
# ⚠️ The published .sig is base64-encoded (that is what `tauri signer sign`
# writes). `minisign -V` refuses it as-is — "untrusted signature comment too
# long" — so it has to be decoded first. The application does the same thing
# before verifying.
if $SKIP_VERIFY; then
  echo "WARNING: signature check skipped (--insecure-skip-verify)."
  echo "         You are installing whatever the network handed you."
else
  echo "Verifying signature..."
  if ! curl -fsSL -o "${TMP_DEB}.sig" "${DOWNLOAD_URL}.sig"; then
    echo "Error: no signature published for ${DEB_FILE} — refusing to install." >&2
    rm -f "$TMP_DEB" "${TMP_DEB}.sig"
    exit 1
  fi
  if ! base64 -d "${TMP_DEB}.sig" > "${TMP_DEB}.minisig" 2>/dev/null; then
    echo "Error: signature file is not readable — refusing to install." >&2
    rm -f "$TMP_DEB" "${TMP_DEB}.sig" "${TMP_DEB}.minisig"
    exit 1
  fi
  if ! minisign -V -P "$PUBKEY" -m "$TMP_DEB" -x "${TMP_DEB}.minisig" >/dev/null 2>&1; then
    echo "Error: SIGNATURE DOES NOT MATCH. The package was not published by" >&2
    echo "       LanProbe, or it was altered in transit. Nothing installed." >&2
    rm -f "$TMP_DEB" "${TMP_DEB}.sig" "${TMP_DEB}.minisig"
    exit 1
  fi
  rm -f "${TMP_DEB}.sig" "${TMP_DEB}.minisig"
  echo "Signature verified."
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

# ── Enrolment ─────────────────────────────────────────────────────────────────
# 🔴 Installing and attaching are two different things, and this is why the
# script asks: the probe listens on no port, so a freshly installed one that is
# not attached does absolutely nothing — silently. People used to copy the
# command from the last lines of this output, which is one more place to make
# a typo at the end of a long day.
#
# ⚠️ Already attached → nothing is asked and nothing is touched. Re-enrolling a
# working probe would create a SECOND probe in the fleet, and the measurements
# of one site would be split across two rows.
#
# ⚠️ The code is read from /dev/tty, not stdin: this script is routinely run as
# `curl … | sudo bash`, where stdin is the script itself.
enrol() {
  if [[ -f "${CONFIG_DIR}/app_config.json" ]] && grep -q '"probe_id"' "${CONFIG_DIR}/app_config.json" 2>/dev/null; then
    echo "  Already attached to a hub — nothing to enrol."
    return 0
  fi

  if [[ -z "$HUB_URL" || -z "$ENROL_CODE" ]]; then
    if [[ -r /dev/tty ]]; then
      echo ""
      echo "  This probe is not attached to a hub yet."
      echo "  Create a code in the hub (Fleet -> + on a site), then paste it here."
      echo "  Leave empty to skip and do it later."
      [[ -z "$HUB_URL" ]] && { printf "  Hub URL  : " > /dev/tty; read -r HUB_URL < /dev/tty; }
      [[ -z "$ENROL_CODE" ]] && { printf "  Code     : " > /dev/tty; read -r ENROL_CODE < /dev/tty; }
    fi
  fi

  if [[ -z "$HUB_URL" || -z "$ENROL_CODE" ]]; then
    echo ""
    echo "  Not attached. When you have a code:"
    echo "    sudo -u lanprobe lanprobe-server --config-dir ${CONFIG_DIR} \\"
    echo "         enroll --hub https://your-hub --code A1B2-C3D4"
    echo ""
    echo "  It shows up in the fleet on its first heartbeat, under a minute."
    return 0
  fi

  echo ""
  echo "Enrolling against ${HUB_URL}..."
  # ⚠️ As the `lanprobe` user, never as root: the config file belongs to it, and
  # a root-owned app_config.json would leave the service unable to write its own
  # state — a probe that enrols successfully and then goes quiet.
  if runuser -u lanprobe -- /usr/bin/lanprobe-server --config-dir "${CONFIG_DIR}" \
       enroll --hub "$HUB_URL" --code "$ENROL_CODE"; then
    echo "Enrolled. Restarting ${SERVICE}..."
    systemctl restart "${SERVICE}" 2>/dev/null || true
    echo "It shows up in the fleet on its first heartbeat, under a minute."
  else
    echo "Enrolment failed — the probe is installed but attached to nothing." >&2
    echo "A code is single-use and lasts 15 minutes; make a fresh one and run:" >&2
    echo "  sudo -u lanprobe lanprobe-server --config-dir ${CONFIG_DIR} \\" >&2
    echo "       enroll --hub <url> --code <code>" >&2
  fi
}

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
  echo "  The probe listens on no port — it is a client of your LanProbe hub."
  enrol
else
  echo ""
  echo "lanprobe-server ${INSTALL_VERSION} installed (no systemd detected)."
  echo "Attach it first: /usr/bin/lanprobe-server --config-dir ${CONFIG_DIR} enroll --hub <url> --code <code>"
fi
