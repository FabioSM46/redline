#!/usr/bin/env bash
# The desktop file is installed under the bundle identifier (#87): the configuration agrees,
# the release runs the rename before uploading, and the rename works on a synthetic package.
#
# Run: bash scripts/test/deb-desktop-id.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/deb-desktop-id.sh"
RELEASE="$ROOT/.github/workflows/release.yml"

pass=0
fail=0
ok() { echo "  ok   — $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL — $1"; fail=$((fail + 1)); }
assert_eq() {
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$2', got '$3'"; fi
}
assert_contains() {
  if [[ "$2" == *"$3"* ]]; then ok "$1"; else bad "$1: expected to find '$3' in:"; printf '           %s\n' "$2"; fi
}
assert_not_contains() {
  if [[ "$2" != *"$3"* ]]; then ok "$1"; else bad "$1: did NOT expect '$3' in:"; printf '           %s\n' "$2"; fi
}

identifier=$(jq -r .identifier "$ROOT/src-tauri/tauri.conf.json")
OLD=usr/share/applications/redline.desktop
NEW="usr/share/applications/$identifier.desktop"

echo "--- configuration ---"
assert_eq "the identifier is the expected one" "com.fabiosm46.redline" "$identifier"
assert_eq "the template is named after the identifier" "linux/$identifier.desktop" \
  "$(jq -r .bundle.linux.deb.desktopTemplate "$ROOT/src-tauri/tauri.conf.json")"
if [ -f "$ROOT/src-tauri/linux/$identifier.desktop" ]; then ok "the template exists"; else bad "the template is missing"; fi
assert_eq "the release configuration does not override the template" "null" \
  "$(jq -c .bundle.linux.deb.desktopTemplate "$ROOT/src-tauri/tauri.release.linux.conf.json")"
assert_eq "the Debian package stays redline" "redline" \
  "$(jq -r .productName "$ROOT/src-tauri/tauri.linux.conf.json")"

build_line=$(grep -n 'pnpm tauri build' "$RELEASE" | head -n 1 | cut -d: -f1)
rename_line=$(grep -n 'bash scripts/deb-desktop-id.sh' "$RELEASE" | head -n 1 | cut -d: -f1)
upload_line=$(grep -n 'path: src-tauri/target/release/bundle/deb/\*.deb' "$RELEASE" | head -n 1 | cut -d: -f1)
if [ -n "$build_line" ] && [ -n "$rename_line" ] && [ -n "$upload_line" ] \
  && [ "$build_line" -lt "$rename_line" ] && [ "$rename_line" -lt "$upload_line" ]; then
  ok "the release renames after building and before uploading"
else
  bad "release.yml order: build '$build_line', rename '$rename_line', upload '$upload_line'"
fi

echo "--- the rename ---"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

make_deb() { # <output> <path of the desktop file, empty for none>
  local pkg="$TMP/pkg"
  rm -rf "$pkg"
  mkdir -p "$pkg/DEBIAN" "$pkg/usr/bin" "$pkg/usr/share/applications"
  printf 'Package: redline\nVersion: 0.1.0\nArchitecture: amd64\nMaintainer: example <noreply@example.com>\nDescription: test\n' >"$pkg/DEBIAN/control"
  printf '#!/bin/sh\n' >"$pkg/usr/bin/redline"
  chmod 755 "$pkg/usr/bin/redline"
  if [ -n "$2" ]; then printf '[Desktop Entry]\nName=Redline\n' >"$pkg/$2"; fi
  (cd "$pkg" && find usr -type f -exec md5sum {} + >DEBIAN/md5sums)
  dpkg-deb -Zgzip --root-owner-group --build "$pkg" "$1" >/dev/null
}

DEB="$TMP/redline_0.1.0_amd64.deb"
make_deb "$DEB" "$OLD"
OUT=$(bash "$SCRIPT" "$DEB" 2>&1)
RC=$?
assert_eq "the rename succeeds" "0" "$RC"
assert_contains "and says what it did" "$OUT" "renamed"

LISTING=$(dpkg-deb -c "$DEB")
assert_contains "the package installs the new name" "$LISTING" "./$NEW"
assert_not_contains "and not the old one" "$LISTING" "./$OLD"
assert_contains "ownership stays root" "$(grep "./$NEW" <<<"$LISTING")" "root/root"
assert_contains "the binary stays executable" "$(grep './usr/bin/redline' <<<"$LISTING")" "-rwxr-xr-x"
assert_eq "the package name is unchanged" "redline" "$(dpkg-deb -f "$DEB" Package)"

CHECK="$TMP/check"
rm -rf "$CHECK"
dpkg-deb --raw-extract "$DEB" "$CHECK"
SUMS=$(cat "$CHECK/DEBIAN/md5sums")
assert_contains "md5sums names the new path" "$SUMS" "  $NEW"
assert_not_contains "md5sums forgets the old one" "$SUMS" "$OLD"
if (cd "$CHECK" && md5sum --check --quiet DEBIAN/md5sums >/dev/null 2>&1); then
  ok "md5sums verifies against the package"
else
  bad "md5sums does not verify"
fi

# Tauri writes the archives itself, with bare paths where dpkg-deb writes `./usr/...`.
TAURI="$TMP/tauri.deb"
make_deb "$TMP/plain.deb" "$OLD"
(
  cd "$TMP/pkg"
  tar --owner=0 --group=0 --numeric-owner -czf "$TMP/data.tar.gz" usr
  tar --owner=0 --group=0 --numeric-owner -C DEBIAN -czf "$TMP/control.tar.gz" control md5sums
  cd "$TMP"
  printf '2.0\n' >debian-binary
  ar rc "$TAURI" debian-binary control.tar.gz data.tar.gz
)
if dpkg-deb --fsys-tarfile "$TAURI" | tar -t | grep -qx "$OLD"; then
  ok "the Tauri-shaped package has bare paths"
else
  bad "the Tauri-shaped package does not have bare paths"
fi
OUT=$(bash "$SCRIPT" "$TAURI" 2>&1)
assert_eq "a package with bare paths is renamed too" "0" "$?"
LISTING=$(dpkg-deb -c "$TAURI")
assert_contains "and installs the new name" "$LISTING" "$NEW"
assert_not_contains "and not the old one" "$LISTING" "$OLD"

BEFORE=$(sha256sum <"$DEB")
OUT=$(bash "$SCRIPT" "$DEB" 2>&1)
assert_eq "a second run succeeds" "0" "$?"
assert_contains "and says so" "$OUT" "already installs"
assert_eq "and leaves the package alone" "$BEFORE" "$(sha256sum <"$DEB")"

NONE="$TMP/none.deb"
make_deb "$NONE" ""
BEFORE=$(sha256sum <"$NONE")
OUT=$(bash "$SCRIPT" "$NONE" 2>&1)
RC=$?
assert_eq "a package without the file fails" "1" "$RC"
assert_contains "and names what is missing" "$OUT" "does not contain"
assert_eq "and stays as it was" "$BEFORE" "$(sha256sum <"$NONE")"

echo
echo "deb-desktop-id: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
