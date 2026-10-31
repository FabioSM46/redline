#!/bin/bash
# Installs Redline's system dependencies from a source checkout:
#   - LACT (the lactd daemon) from the official package of the pinned version, checksum verified,
#     or from the distribution with pacman
#   - GPU Screen Recorder via Flatpak, system-wide (needed to capture a monitor on AMD)
#   - ffmpeg from the distribution's package manager (previews of clips)
#   - MangoHud from the distribution's package manager (in-game overlay)
# It runs the same helper the .deb ships at /usr/share/redline/install-dependency, which the
# app starts through pkexec: the pinned versions and checksums live only in that helper.
# Usage: sudo bash scripts/install-deps.sh [--dry-run] [lact] [gpu-screen-recorder] [ffmpeg]
#        [mangohud] (all four when none is named).
set -euo pipefail

HELPER="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/src-tauri/linux/install-dependency"

options=()
if [ "${1:-}" = --dry-run ]; then
  options+=(--dry-run)
  shift
fi
[ $# -gt 0 ] || set -- lact gpu-screen-recorder ffmpeg mangohud

# A component this system does not support (the helper's exit code 3, for example LACT on a
# system without a package for it) is skipped; any other failure stops the run with the helper's code.
for component in "$@"; do
  echo "==> $component" >&2
  status=0
  bash "$HELPER" "${options[@]}" "$component" || status=$?
  case $status in
    0) ;;
    3) echo "  skipped: not supported on this system" >&2 ;;
    *) exit "$status" ;;
  esac
done

echo "Done. If this is the first Flatpak install, log out and back in to see the apps in the menu." >&2
