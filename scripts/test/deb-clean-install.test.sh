#!/usr/bin/env bash
# The release installs the package on clean systems before the draft (#128): the workflow keeps
# that shape, and the script checks what the release configuration ships. The install itself
# needs a container and belongs to the release path, so it does not run here.
#
# Run: bash scripts/test/deb-clean-install.test.sh

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/deb-clean-install.sh"

bash -n "$SCRIPT"
bash -n "$ROOT/scripts/distro-clean-install.sh"

python3 - "$ROOT" <<'PY'
import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
workflow = (root / ".github/workflows/release.yml").read_text()
script = (root / "scripts/deb-clean-install.sh").read_text()
distro = (root / "scripts/distro-clean-install.sh").read_text()


def job(name):
    match = re.search(rf"^  {name}:\n((?:(?:    .*)?\n)+)", workflow, re.MULTILINE)
    assert match, f"release.yml has no job {name}"
    return match.group(1)


def needs(body):
    match = re.search(r"^    needs: \[?([^\]\n]*)\]?$", body, re.MULTILINE)
    assert match, "the job has no needs"
    return {item.strip() for item in match.group(1).split(",")}


check = job("install-check")
draft = job("draft")

assert needs(check) == {"linux"}, "install-check must run after the build"
assert {"linux", "install-check", "distro"} <= needs(draft), "a failed install must stop the draft"
assert "merge-multiple: true" in draft, "the draft must publish every package artifact"

images = re.search(r"^        image: \[(.*)\]$", check, re.MULTILINE)
assert images, "install-check needs a matrix of images"
assert [item.strip().strip('"') for item in images.group(1).split(",")] == [
    "ubuntu:24.04",
    "ubuntu:26.04",
    "debian:13",
], "install-check must cover exactly the supported systems"

assert re.search(r"^      fail-fast: false$", check, re.MULTILINE), (
    "one failing system must not cancel the others"
)
assert "name: linux-deb" in check, "install-check must test the artifact the draft publishes"
assert "bash /check/deb-clean-install.sh" in check, "install-check must run the script"
assert "scripts:/check:ro" in check and "packages:/packages:ro" in check, (
    "the mounts must be read-only"
)

# `contents: write` stays on the draft job alone.
assert "permissions:" not in check, "install-check needs no permission of its own"
assert workflow.count("contents: write") == 1 and "contents: write" in draft

# What the release configuration ships beside the bundle is what the script expects.
config = json.loads((root / "src-tauri/tauri.release.linux.conf.json").read_text())
identifier = json.loads((root / "src-tauri/tauri.conf.json").read_text())["identifier"]
shipped = set(config["bundle"]["linux"]["deb"]["files"])
expected = set(re.findall(r"^  (/usr/\S+)$", script, re.MULTILINE))
assert expected == shipped | {f"/usr/share/applications/{identifier}.desktop"}, (
    f"expected files differ from the release configuration: {sorted(expected ^ shipped)}"
)

for needle in ("dpkg -V redline", 'ldd "$BIN"', "/etc/dpkg/dpkg.cfg.d", "/.dockerenv", "ldconfig -p"):
    assert needle in script, f"the script lost {needle!r}"

for needle in ('dpkg-deb -f "$DEB" Depends', "package_declares deb curl", "command -v curl"):
    assert needle in script, f"the script lost {needle!r}"
for needle in (
    "pacman -Qi redline",
    "rpm -q --requires redline",
    'package_declares "$system" curl',
    "command -v curl",
):
    assert needle in distro, f"the distro script lost {needle!r}"

print("deb clean install — workflow shape and expected files agree")
PY

# shellcheck source=scripts/package-dependency.sh
. "$ROOT/scripts/package-dependency.sh"

# declared <format> <name> <text>: the text must declare the name; undeclared: it must not.
declared() {
  package_declares "$1" "$2" <<<"$3" || { echo "FAIL: $1 should declare $2 in: $3"; exit 1; }
}
undeclared() {
  if package_declares "$1" "$2" <<<"$3"; then
    echo "FAIL: $1 should not declare $2 in: $3"
    exit 1
  fi
}

declared deb curl 'libc6 (>= 2.35), curl, libgtk-3-0'
declared deb curl 'curl'
declared deb curl 'libc6, curl (>= 7.0)'
declared deb curl 'libc6, curl:any'
declared deb curl 'curl | wget, libc6'
declared deb curl $'libc6,\n curl,\n libgtk-3-0'
undeclared deb curl 'libc6 (>= 2.35), libgtk-3-0'
undeclared deb curl ''
undeclared deb curl 'libcurl4, python-curl, curl-config'
undeclared deb curl 'libcurl4 (>= 7.0), curl-doc (= curl)'
undeclared deb curl 'wget | curl'

declared fedora curl $'/bin/sh\ncurl\nlibgtk-3.so.0()(64bit)'
declared fedora curl $'curl >= 7.0\nrpmlib(CompressedFileNames) <= 3.0.4-1'
undeclared fedora curl $'/bin/sh\nlibcurl.so.4()(64bit)\nlibcurl\npython3-curl'
undeclared fedora curl ''

arch_info() { printf 'Name            : redline\nDepends On      : %s\nOptional Deps   : curl\n' "$1"; }
declared arch curl "$(arch_info 'gtk3  curl  webkit2gtk-4.1')"
declared arch curl "$(arch_info 'gtk3  curl>=7.0')"
declared arch curl "$(printf 'Depends On      : gtk3\n                  curl\nProvides        : None\n')"
undeclared arch curl "$(arch_info 'gtk3  libcurl-gnutls  python-curl')"
undeclared arch curl "$(arch_info 'None')"
undeclared arch curl ''

status=0
package_declares rpm curl <<<'curl' 2>/dev/null || status=$?
[ "$status" -eq 2 ] || { echo "FAIL: an unknown format must be an error, got $status"; exit 1; }

echo "deb clean install — the dependency check matches curl by name"

# Only where the script is meant to stop: inside a container it would really install.
if [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ]; then
  deb=$(mktemp)
  trap 'rm -f "$deb"' EXIT
  status=0
  bash "$SCRIPT" "$deb" 2>/dev/null || status=$?
  [ "$status" -eq 2 ] || { echo "FAIL: expected a refusal outside a container, got $status"; exit 1; }
  echo "deb clean install — refuses to run outside a container"
fi
