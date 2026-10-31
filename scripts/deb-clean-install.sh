#!/usr/bin/env bash
# Installs a built .deb on the clean system of this container and checks the result (#128).
#
# It runs INSIDE a throwaway container, as root, and it changes that system: the release job
# starts it once per supported system, with the package mounted read-only.
#
#   docker run --rm -v "$PWD/packages:/packages:ro" -v "$PWD/scripts:/check:ro" <image> \
#     bash /check/deb-clean-install.sh /packages/<file>.deb
#
# The binary has no --version: every launch starts the GUI and unknown arguments are ignored
# (src-tauri/src/actions.rs). The headless check is therefore `ldd`, on the fallback the issue
# names. A container has no desktop session, no pkexec agent and no GPU: passing here means the
# package installs and its libraries resolve, not that the app works.

set -euo pipefail

BIN=/usr/bin/redline
TRAY_LIB=libayatana-appindicator3.so.1
EXPECTED_FILES=(
  /usr/share/applications/com.fabiosm46.redline.desktop
  /usr/share/polkit-1/actions/com.fabiosm46.redline.install.policy
  /usr/share/redline/install-dependency
  /usr/share/doc/redline/copyright
  /usr/share/doc/redline/THIRD-PARTY-LICENSES
)
RETIRED_FILES=(/usr/share/applications/redline.desktop)

[ "$#" -eq 1 ] || { echo "usage: $0 <package.deb>" >&2; exit 2; }
DEB=$1

# This script edits the dpkg configuration and installs packages: never on a real machine.
if [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ]; then
  echo "error: this script changes the system; run it in a container" >&2
  exit 2
fi
[ -f "$DEB" ] || { echo "error: no package at $DEB" >&2; exit 2; }

# Ubuntu container images ship path-exclude rules that drop /usr/share/doc/* at unpack time,
# which no installed system has: with them `dpkg -V` reports THIRD-PARTY-LICENSES as missing and
# the license check fails for a reason unrelated to the package. Debian's image has no such
# rules. Every rule goes, under whatever file name, so a new image cannot bring them back.
if [ -d /etc/dpkg/dpkg.cfg.d ]; then
  find /etc/dpkg/dpkg.cfg.d -type f -exec sed -i '/^path-\(exclude\|include\)=/d' {} +
fi

export DEBIAN_FRONTEND=noninteractive
apt-get -o Acquire::Retries=3 update
(cd "$(dirname "$DEB")" && apt -o Acquire::Retries=3 install -y "./$(basename "$DEB")")

failed=0
fail() { echo "FAILED: $1" >&2; failed=1; }

# Without md5sums `dpkg -V` has nothing to compare and prints nothing, which would read as a pass.
[ -s /var/lib/dpkg/info/redline.md5sums ] || fail "the package has no md5sums"

verify_status=0
verify=$(dpkg -V redline 2>&1) || verify_status=$?
[ "$verify_status" -eq 0 ] || fail "dpkg -V exited $verify_status"
[ -z "$verify" ] || fail "dpkg -V printed: $verify"

for file in "${EXPECTED_FILES[@]}"; do
  [ -s "$file" ] || fail "$file is missing or empty"
done
for file in "${RETIRED_FILES[@]}"; do
  [ ! -e "$file" ] || fail "$file should not be installed"
done
[ -x /usr/share/redline/install-dependency ] || fail "the install helper is not executable"
[ -x "$BIN" ] || fail "$BIN is not executable"

ldd_status=0
libs=$(ldd "$BIN" 2>&1) || ldd_status=$?
[ "$ldd_status" -eq 0 ] || fail "ldd exited $ldd_status"
if unresolved=$(grep -F 'not found' <<<"$libs"); then
  fail "unresolved libraries: $unresolved"
fi

# The tray library is not linked: libappindicator-sys loads it at run time, so `ldd` never sees
# it. Only the package's Depends brings it in.
# Not a pipe: grep -q closing it early would fail the pipeline under pipefail.
grep -Fq "$TRAY_LIB" <<<"$(ldconfig -p)" || fail "$TRAY_LIB is not installed"

. /etc/os-release
if [ "$failed" -ne 0 ]; then
  echo "Clean install check FAILED on $PRETTY_NAME" >&2
  exit 1
fi
echo "Clean install check passed on $PRETTY_NAME: $(wc -l <<<"$libs") libraries resolved"
