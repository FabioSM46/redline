#!/usr/bin/env bash
# Installs a built Redline into a package root: the same files as the .deb, shared by the Arch
# and Fedora packages. Paths must match HELPER in src-tauri/src/installer.rs.
#
#   bash packaging/install-files.sh <repo-root> <package-root>

set -euo pipefail

src=$1/src-tauri
dest=$2
identifier=$(jq -er .identifier "$src/tauri.conf.json")

install -Dm755 "$src/target/release/redline" "$dest/usr/bin/redline"
install -Dm755 "$src/linux/install-dependency" "$dest/usr/share/redline/install-dependency"
install -Dm644 "$src/linux/$identifier.install.policy" \
  "$dest/usr/share/polkit-1/actions/$identifier.install.policy"
install -Dm644 /dev/stdin "$dest/usr/share/applications/$identifier.desktop" < <(
  sed -e 's/{{comment}}/Control panel for AMD GPUs on Linux/' -e 's/{{exec}}/redline/' \
    -e 's/{{icon}}/redline/' "$src/linux/$identifier.desktop"
)
for size in 32x32 128x128; do
  install -Dm644 "$src/icons/$size.png" "$dest/usr/share/icons/hicolor/$size/apps/redline.png"
done
install -Dm644 "$src/icons/icon.png" "$dest/usr/share/icons/hicolor/512x512/apps/redline.png"
install -Dm644 "$1/LICENSE" "$dest/usr/share/licenses/redline/LICENSE"
install -Dm644 "$src/target/THIRD-PARTY-LICENSES" \
  "$dest/usr/share/licenses/redline/THIRD-PARTY-LICENSES"
