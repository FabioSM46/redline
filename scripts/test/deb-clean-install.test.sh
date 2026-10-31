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

python3 - "$ROOT" <<'PY'
import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1])
workflow = (root / ".github/workflows/release.yml").read_text()
script = (root / "scripts/deb-clean-install.sh").read_text()


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
assert {"linux", "install-check"} <= needs(draft), "a failed install must stop the draft"

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

print("deb clean install — workflow shape and expected files agree")
PY

# Only where the script is meant to stop: inside a container it would really install.
if [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ]; then
  deb=$(mktemp)
  trap 'rm -f "$deb"' EXIT
  status=0
  bash "$SCRIPT" "$deb" 2>/dev/null || status=$?
  [ "$status" -eq 2 ] || { echo "FAIL: expected a refusal outside a container, got $status"; exit 1; }
  echo "deb clean install — refuses to run outside a container"
fi
