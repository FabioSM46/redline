#!/usr/bin/env bash
# Installs a built Arch or Fedora package on the clean system of this container and checks the
# result, as scripts/deb-clean-install.sh does for the .deb. Passing means the package installs
# and its libraries resolve, not that the app works.
#
#   docker run --rm -v "$PWD/packages:/packages:ro" -v "$PWD/scripts:/check:ro" <image> \
#     bash /check/distro-clean-install.sh arch|fedora /packages/<file>

set -euo pipefail

BIN=/usr/bin/redline
TRAY_LIB=libayatana-appindicator3.so.1
EXPECTED_FILES=(
  /usr/share/applications/com.fabiosm46.redline.desktop
  /usr/share/polkit-1/actions/com.fabiosm46.redline.install.policy
  /usr/share/redline/install-dependency
  /usr/share/licenses/redline/LICENSE
  /usr/share/licenses/redline/THIRD-PARTY-LICENSES
)

[ "$#" -eq 2 ] || { echo "usage: $0 arch|fedora <package>" >&2; exit 2; }
system=$1 package=$2

if [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ]; then
  echo "error: this script changes the system; run it in a container" >&2
  exit 2
fi
[ -f "$package" ] || { echo "error: no package at $package" >&2; exit 2; }

case $system in
  arch)
    pacman -Syu --noconfirm
    pacman -U --noconfirm "$package"
    pacman -Qkk redline
    ;;
  fedora)
    dnf install -y "$package" /usr/bin/ldconfig
    rpm -V redline
    ;;
  *)
    echo "usage: $0 arch|fedora <package>" >&2
    exit 2
    ;;
esac

failed=0
fail() { echo "FAILED: $1" >&2; failed=1; }

for file in "${EXPECTED_FILES[@]}"; do
  [ -e "$file" ] || fail "$file is missing"
done

libraries=$(ldd "$BIN" 2>&1) || fail "ldd could not read $BIN"
if grep -F 'not found' <<<"$libraries"; then
  fail "a library of $BIN does not resolve"
fi
# Loaded at run time, so it is not in the binary's NEEDED list and ldd cannot see it.
# Not a pipe: grep -q closing it early would fail the pipeline under pipefail.
grep -Fq "$TRAY_LIB" <<<"$(ldconfig -p)" || fail "$TRAY_LIB is not installed"

[ "$failed" -eq 0 ] || exit 1
echo "Clean install check passed on $system: $(wc -l <<<"$libraries") libraries resolved"
