#!/usr/bin/env bash
# Installs the desktop file of a built .deb under the bundle identifier (#87).
#
# Tauri's deb bundler names the file `{productName}.desktop` and has no option for it.
# productName cannot be the identifier on Linux: it also gives the Debian package name, the
# package file name and /usr/lib/<productName>. So the bundler keeps `redline` and this script
# renames the file inside the finished package, updating `md5sums` to match.
#
# Run after `pnpm tauri build`, on the default folder or on the packages given:
#   bash scripts/deb-desktop-id.sh [package.deb ...]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
identifier=$(jq -er .identifier "$ROOT/src-tauri/tauri.conf.json")
product=$(jq -er .productName "$ROOT/src-tauri/tauri.linux.conf.json")
OLD="usr/share/applications/$product.desktop"
NEW="usr/share/applications/$identifier.desktop"

if [ "$#" -eq 0 ]; then
  shopt -s nullglob
  set -- "$ROOT"/src-tauri/target/release/bundle/deb/*.deb
  shopt -u nullglob
  [ "$#" -gt 0 ] || { echo "error: no .deb in the bundle folder" >&2; exit 1; }
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

lists() { grep -Fxq "$2" <<<"$1"; }

for deb in "$@"; do
  # Tauri's archive has bare paths, dpkg-deb's has a leading `./`: this reads both.
  listing=$(dpkg-deb --fsys-tarfile "$deb" | tar -t | sed 's#^\./##')
  if lists "$listing" "$NEW" && ! lists "$listing" "$OLD"; then
    echo "$deb: already installs $NEW"
    continue
  fi
  lists "$listing" "$OLD" || { echo "error: $deb does not contain $OLD" >&2; exit 1; }

  rm -rf "$WORK/package" "$WORK/renamed.deb"
  dpkg-deb --raw-extract "$deb" "$WORK/package"
  mv "$WORK/package/$OLD" "$WORK/package/$NEW"
  # `md5sums` is optional in a package, and its paths may have a leading `./` or not.
  if [ -f "$WORK/package/DEBIAN/md5sums" ]; then
    OLD="$OLD" NEW="$NEW" awk '
      BEGIN { from = ENVIRON["OLD"]; to = ENVIRON["NEW"] }
      {
        path = $0
        sub(/^[0-9a-f]+  /, "", path)
        bare = path
        sub(/^\.\//, "", bare)
        if (bare == from) $0 = substr($0, 1, length($0) - length(path)) to
        print
      }
    ' "$WORK/package/DEBIAN/md5sums" >"$WORK/md5sums"
    cat "$WORK/md5sums" >"$WORK/package/DEBIAN/md5sums"
  fi
  dpkg-deb -Zgzip --root-owner-group --build "$WORK/package" "$WORK/renamed.deb" >/dev/null
  # `cat` and not `mv`: the package keeps its path and the caller's permissions.
  cat "$WORK/renamed.deb" >"$deb"
  echo "$deb: renamed $OLD to $NEW"
done
