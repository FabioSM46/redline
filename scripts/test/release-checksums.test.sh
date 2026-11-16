#!/usr/bin/env bash
# The release publishes SHA256SUMS with its packages (#157): the script writes a file
# `sha256sum -c` accepts, refuses one that does not list exactly the packages, and the draft
# job calls it before and after the upload.
#
# Run: bash scripts/test/release-checksums.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/release-checksums.sh"
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

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

DEB=redline_0.1.0_amd64.deb
ARCH=redline-0.1.0-1-x86_64.pkg.tar.zst
RPM=redline-0.1.0-1.fc44.x86_64.rpm

# A fresh folder with the three fake packages.
packages() { # <name>
  local dir="$TMP/$1"
  mkdir -p "$dir"
  printf 'deb\n' >"$dir/$DEB"
  printf 'arch\n' >"$dir/$ARCH"
  printf 'rpm\n' >"$dir/$RPM"
  echo "$dir"
}

# run <mode> <folder> [stdin]: sets OUT and RC.
run() {
  OUT=$(bash "$SCRIPT" "$1" "$2" 2>&1 <<<"${3-}")
  RC=$?
}

echo "--- write ---"
DIR=$(packages write)
run write "$DIR"
assert_eq "write succeeds" "0" "$RC"
assert_eq "one line per package" "3" "$(wc -l <"$DIR/SHA256SUMS")"
assert_eq "the names are sorted and have no folder" "$ARCH $RPM $DEB" \
  "$(sed 's/^[0-9a-f]\{64\}  //' "$DIR/SHA256SUMS" | paste -sd ' ')"
assert_eq "SHA256SUMS does not list itself" "0" "$(grep -c 'SHA256SUMS' "$DIR/SHA256SUMS")"
assert_eq "the checksum is the package's" "$(sha256sum <"$DIR/$DEB" | cut -d' ' -f1)" \
  "$(grep "  $DEB\$" "$DIR/SHA256SUMS" | cut -d' ' -f1)"
if (cd "$DIR" && sha256sum -c --strict SHA256SUMS >/dev/null 2>&1); then
  ok "sha256sum -c accepts the file"
else
  bad "sha256sum -c rejects the file"
fi

# What the README tells a user to do: one package and SHA256SUMS in a folder.
mkdir "$TMP/download"
cp "$DIR/$DEB" "$DIR/SHA256SUMS" "$TMP/download/"
if (cd "$TMP/download" && sha256sum -c --ignore-missing SHA256SUMS >/dev/null 2>&1); then
  ok "a single download verifies with --ignore-missing"
else
  bad "a single download does not verify with --ignore-missing"
fi
printf 'tampered\n' >"$TMP/download/$DEB"
if (cd "$TMP/download" && sha256sum -c --ignore-missing SHA256SUMS >/dev/null 2>&1); then
  bad "a changed download still verifies"
else
  ok "a changed download does not verify"
fi

FIRST=$(cat "$DIR/SHA256SUMS")
run write "$DIR"
assert_eq "a second write succeeds" "0" "$RC"
assert_eq "and writes the same file" "$FIRST" "$(cat "$DIR/SHA256SUMS")"

printf 'rebuilt\n' >"$DIR/$DEB"
run write "$DIR"
assert_eq "a rebuilt package is written again" "0" "$RC"
assert_eq "with its new checksum" "$(sha256sum <"$DIR/$DEB" | cut -d' ' -f1)" \
  "$(grep "  $DEB\$" "$DIR/SHA256SUMS" | cut -d' ' -f1)"

echo "--- check ---"
run check "$DIR"
assert_eq "check accepts what write wrote" "0" "$RC"
assert_contains "and prints the file" "$OUT" "  $DEB"

DIR=$(packages extra)
run write "$DIR"
printf 'late\n' >"$DIR/redline_0.1.0_arm64.deb"
run check "$DIR"
assert_eq "a package SHA256SUMS does not list fails" "1" "$RC"
assert_contains "and is named" "$OUT" "only in the folder: redline_0.1.0_arm64.deb"

DIR=$(packages gone)
run write "$DIR"
rm "$DIR/$RPM"
run check "$DIR"
assert_eq "a listed package that is not there fails" "1" "$RC"
assert_contains "and is named" "$OUT" "only in SHA256SUMS: $RPM"

DIR=$(packages changed)
run write "$DIR"
printf 'changed\n' >"$DIR/$ARCH"
run check "$DIR"
assert_eq "a package changed after the write fails" "1" "$RC"
assert_contains "and says a checksum does not match" "$OUT" "does not match"

DIR=$(packages twice)
run write "$DIR"
LINE=$(grep "  $DEB\$" "$DIR/SHA256SUMS")
printf '%s\n' "$LINE" >>"$DIR/SHA256SUMS"
run check "$DIR"
assert_eq "a package listed twice fails" "1" "$RC"
assert_contains "and is named" "$OUT" "more than once: $DEB"

DIR=$(packages malformed)
run write "$DIR"
echo "not a checksum line" >>"$DIR/SHA256SUMS"
run check "$DIR"
assert_eq "a malformed line fails" "1" "$RC"

DIR=$(packages pathy)
run write "$DIR"
sed -i "s#  $DEB\$#  ./$DEB#" "$DIR/SHA256SUMS"
run check "$DIR"
assert_eq "a name with a folder fails" "1" "$RC"

DIR=$(packages missing)
run check "$DIR"
assert_eq "check without SHA256SUMS fails" "1" "$RC"
assert_contains "and says it is missing" "$OUT" "is missing"

echo "--- what the folder may hold ---"
mkdir "$TMP/empty"
run write "$TMP/empty"
assert_eq "an empty folder fails" "1" "$RC"
assert_contains "and says so" "$OUT" "holds no package"
if [ -e "$TMP/empty/SHA256SUMS" ]; then bad "and still wrote a file"; else ok "and writes no file"; fi

DIR=$(packages nested)
mkdir "$DIR/linux-deb"
run write "$DIR"
assert_eq "a subfolder fails" "1" "$RC"
assert_contains "and is named" "$OUT" "linux-deb is not a regular file"

DIR=$(packages spaced)
printf 'x\n' >"$DIR/redline 0.1.0.deb"
run write "$DIR"
assert_eq "a name with a space fails" "1" "$RC"

DIR=$(packages tilde)
printf 'x\n' >"$DIR/redline_0.2.0~beta1_amd64.deb"
run write "$DIR"
assert_eq "a name with a tilde, which a release would store under another name, fails" "1" "$RC"
assert_contains "and is named" "$OUT" "unexpected package name: redline_0.2.0~beta1_amd64.deb"

DIR=$(packages hidden)
printf 'x\n' >"$DIR/.hidden"
run write "$DIR"
assert_eq "a hidden file fails" "1" "$RC"

run write "$TMP/absent"
assert_eq "a folder that does not exist fails" "1" "$RC"
run sign "$TMP/empty"
assert_eq "an unknown mode is a usage error" "2" "$RC"
OUT=$(bash "$SCRIPT" write 2>&1)
assert_eq "a missing folder argument is a usage error" "2" "$?"

echo "--- assets ---"
DIR=$(packages assets)
run write "$DIR"
run assets "$DIR" "$(printf '%s\n' "$RPM" SHA256SUMS "$DEB" "$ARCH")"
assert_eq "the packages and SHA256SUMS, in any order, pass" "0" "$RC"

run assets "$DIR" "$(printf '%s\n' "$RPM" SHA256SUMS "$DEB" "$ARCH" redline-0.1.0-1.fc43.x86_64.rpm)"
assert_eq "an asset left from an earlier run fails" "1" "$RC"
assert_contains "and is named" "$OUT" "only in the release: redline-0.1.0-1.fc43.x86_64.rpm"
assert_contains "and says how to recover" "$OUT" "delete from the draft"

run assets "$DIR" "$(printf '%s\n' SHA256SUMS "$DEB" "$ARCH")"
assert_eq "a package that was not uploaded fails" "1" "$RC"
assert_contains "and is named" "$OUT" "$RPM"

run assets "$DIR" "$(printf '%s\n' "$RPM" "$DEB" "$ARCH")"
assert_eq "a release without SHA256SUMS fails" "1" "$RC"

run assets "$DIR" ""
assert_eq "an empty asset list fails" "1" "$RC"

echo "--- the draft job ---"
if python3 - "$RELEASE" <<'PY'
import re
import sys
from pathlib import Path

workflow = Path(sys.argv[1]).read_text()
match = re.search(r"^  draft:\n((?:(?:    .*)?\n)+)", workflow, re.MULTILINE)
assert match, "release.yml has no draft job"
draft = match.group(1)


def at(needle):
    position = draft.find(needle)
    assert position >= 0, f"the draft job lost {needle!r}"
    return position


checkout = at("actions/checkout@")
download = at("actions/download-artifact@")
write = at("bash scripts/release-checksums.sh write packages")
check = at("bash scripts/release-checksums.sh check packages")
upload = at('gh release upload "$TAG" packages/*')
create = at('gh release create "$TAG" packages/*')
assets = at("bash scripts/release-checksums.sh assets packages")

assert checkout < download, "checkout empties the workspace: it must come before the download"
assert download < write < check, "SHA256SUMS is written after the download and checked after that"
assert check < upload and check < create, "the check must run before anything is uploaded"
assert upload < assets and create < assets, "the assets are compared after the upload"
assert "--clobber" in draft, "an existing draft must have its files replaced"
assert "--json assets --jq '.assets[].name'" in draft, "the assets check reads the release"
assert "set -euo pipefail" in draft, "a failed read of the release must fail the step"

# The logic lives in the script: the workflow never computes a checksum itself.
assert "sha256sum" not in workflow, "release.yml must call the script, not sha256sum"
assert workflow.count("release-checksums.sh") == draft.count("release-checksums.sh"), (
    "only the draft job runs the script"
)
PY
then
  ok "the draft job writes, checks, uploads and compares, in that order"
else
  bad "the draft job does not call the script as expected"
fi

echo
echo "release-checksums: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
