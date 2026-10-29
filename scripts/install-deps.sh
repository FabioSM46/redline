#!/bin/bash
# Installs Redline's system dependencies on Ubuntu 24.04:
#   - LACT (the lactd daemon) from the official .deb
#   - GPU Screen Recorder via Flatpak, system-wide (needed to capture a monitor on AMD)
# Usage: sudo bash scripts/install-deps.sh
set -e

LACT_VERSION="0.10.1"
LACT_DEB="lact-${LACT_VERSION}-0.amd64.ubuntu-2404.deb"
LACT_URL="https://github.com/ilya-zlobintsev/LACT/releases/download/v${LACT_VERSION}/${LACT_DEB}"
GSR_ID="com.dec05eba.gpu_screen_recorder"

if [ "$(id -u)" -ne 0 ]; then
  echo "Run this script with sudo" >&2
  exit 1
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "==> LACT ${LACT_VERSION}" >&2
curl -fsSL -o "$TMP/$LACT_DEB" "$LACT_URL"
apt-get install -y "$TMP/$LACT_DEB"
systemctl enable --now lactd

echo "==> Flatpak + Flathub" >&2
apt-get install -y flatpak
flatpak remote-add --if-not-exists --system flathub https://dl.flathub.org/repo/flathub.flatpakrepo

echo "==> GPU Screen Recorder" >&2
flatpak install -y --system flathub "$GSR_ID"

echo "Done. If this is the first Flatpak install, log out and back in to see the apps in the menu." >&2
