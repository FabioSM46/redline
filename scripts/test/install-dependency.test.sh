#!/usr/bin/env bash
# The guided install's helper (src-tauri/linux/install-dependency), run in --dry-run against
# a fake os-release and a PATH of stubs: which commands it would run on each system, that it
# refuses every component name but the four fixed ones, and that the pinned values agree
# across the helper, redline-core, the app, the polkit policy and the .deb configuration.
#
# Run: bash scripts/test/install-dependency.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HELPER="$ROOT/src-tauri/linux/install-dependency"
POLICY="$ROOT/src-tauri/linux/com.fabiosm46.redline.install.policy"
INSTALLED_AT=/usr/share/redline/install-dependency
POLICY_AT=/usr/share/polkit-1/actions/com.fabiosm46.redline.install.policy

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

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# A PATH with only the tools the helper needs, plus the stubs a case asks for: the machine's
# own package manager must not decide the outcome.
make_path() {
  local dir="$TMP/bin-$1" tool
  shift
  rm -rf "$dir"
  mkdir -p "$dir"
  for tool in bash grep head awk mktemp rm id cat dirname; do
    ln -s "$(command -v "$tool")" "$dir/$tool"
  done
  for tool in "$@"; do
    printf '#!/bin/sh\necho "stub %s ran" >&2\nexit 1\n' "$tool" >"$dir/$tool"
    chmod +x "$dir/$tool"
  done
  echo "$dir"
}

os_release() {
  printf '%s\n' "$@" >"$TMP/os-release"
}

# Runs the helper in a dry run; sets OUT (stdout) and CODE.
dry_run() {
  local path=$1
  shift
  OUT=$(PATH="$path" REDLINE_OS_RELEASE="$TMP/os-release" bash "$HELPER" --dry-run "$@" 2>/dev/null)
  CODE=$?
}

echo "== LACT"
apt_path=$(make_path apt apt-get curl systemctl flatpak)
os_release 'ID=ubuntu' 'VERSION_ID="24.04"' 'UBUNTU_CODENAME=noble'
dry_run "$apt_path" lact
assert_eq "Ubuntu 24.04 installs LACT" 0 "$CODE"
deb_line=$(grep -F 'ubuntu-2404.deb' "$HELPER" | head -n 1)
sha=$(awk '{ print $3 }' <<<"$deb_line")
assert_contains "downloads the pinned .deb over https, redirects included" "$OUT" "run curl -fsSL --proto =https --proto-redir =https -o"
assert_contains "from the pinned release" "$OUT" "/releases/download/v0.10.1/lact-0.10.1-0.amd64.ubuntu-2404.deb"
assert_contains "checks the pinned checksum" "$OUT" "sha256sum -c ($sha lact-0.10.1-0.amd64.ubuntu-2404.deb)"
assert_contains "refreshes the package index first" "$OUT" "run apt-get -o DPkg::Lock::Timeout=300 update"
assert_contains "apt installs the downloaded file without questions" "$OUT" "run env DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y /"
assert_contains "enables lactd" "$OUT" "run systemctl enable --now lactd"
assert_eq "the last line says it is done" "progress Done" "$(tail -n 1 <<<"$OUT")"
assert_not_contains "every line is a protocol line" "$OUT" $'\n=='

os_release 'ID=linuxmint' 'VERSION_ID="22"' 'UBUNTU_CODENAME=noble'
dry_run "$apt_path" lact
assert_eq "a derivative of Ubuntu 24.04 installs LACT" 0 "$CODE"
assert_contains "with the Ubuntu 24.04 package" "$OUT" "ubuntu-2404.deb"

os_release 'ID=debian' 'VERSION_ID="13"'
dry_run "$apt_path" lact
assert_contains "Debian 13 gets its own package" "$OUT" "lact-0.10.1-0.amd64.debian-13.deb"

os_release 'ID=ubuntu' 'VERSION_ID="22.04"' 'UBUNTU_CODENAME=jammy'
dry_run "$apt_path" lact
assert_eq "a release without a LACT package is not supported" 3 "$CODE"
assert_contains "and says so" "$OUT" "error There is no LACT 0.10.1 package for this system"
assert_not_contains "without downloading anything" "$OUT" "run curl"

os_release 'ID=fedora' 'VERSION_ID=43'
dry_run "$(make_path dnf dnf)" lact
assert_eq "LACT without apt is not supported" 3 "$CODE"
assert_contains "and says what to do" "$OUT" "error The guided install of LACT needs apt"

echo "== GPU Screen Recorder"
os_release 'ID=ubuntu' 'VERSION_ID="24.04"'
dry_run "$apt_path" gpu-screen-recorder
assert_eq "installs with flatpak present" 0 "$CODE"
assert_not_contains "without installing flatpak again" "$OUT" "install -y flatpak"
assert_contains "adds Flathub system-wide" "$OUT" "run flatpak remote-add --if-not-exists --system flathub https://dl.flathub.org/repo/flathub.flatpakrepo"
assert_contains "installs the app system-wide" "$OUT" "run flatpak install -y --noninteractive --system flathub com.dec05eba.gpu_screen_recorder"

dry_run "$(make_path pacman pacman)" gpu-screen-recorder
assert_contains "installs flatpak first when it is missing" "$OUT" "run pacman -S --needed --noconfirm flatpak"

echo "== ffmpeg"
dry_run "$apt_path" ffmpeg
assert_contains "apt" "$OUT" "run env DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y ffmpeg"
dry_run "$(make_path dnf dnf)" ffmpeg
assert_contains "dnf installs Fedora's ffmpeg-free" "$OUT" "run dnf install -y ffmpeg-free"
dry_run "$(make_path pacman pacman)" ffmpeg
assert_contains "pacman" "$OUT" "run pacman -S --needed --noconfirm ffmpeg"
dry_run "$(make_path none)" ffmpeg
assert_eq "no package manager is not supported" 3 "$CODE"

echo "== MangoHud"
dry_run "$apt_path" mangohud
assert_eq "apt installs it" 0 "$CODE"
assert_contains "from the distribution" "$OUT" "run env DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y mangohud"
assert_eq "the last line says it is done" "progress Done" "$(tail -n 1 <<<"$OUT")"
dry_run "$(make_path dnf dnf)" mangohud
assert_contains "dnf" "$OUT" "run dnf install -y mangohud"
dry_run "$(make_path pacman pacman)" mangohud
assert_contains "pacman" "$OUT" "run pacman -S --needed --noconfirm mangohud"
dry_run "$(make_path none)" mangohud
assert_eq "no package manager is not supported" 3 "$CODE"
assert_contains "and says so" "$OUT" "error No supported package manager"

echo "== Only fixed names"
for arg in "" "LACT" "../lact" "https://example.com/x.deb" "/bin/sh" "lact;id" "--dry-run"; do
  dry_run "$apt_path" "$arg"
  assert_eq "refuses '$arg'" 2 "$CODE"
  assert_not_contains "runs nothing for '$arg'" "$OUT" "run "
done
dry_run "$apt_path" lact ffmpeg
assert_eq "refuses two components" 2 "$CODE"
for arg in $(sed -n 's/.*DependencyId::[A-Za-z]* => "\([a-z-]*\)",$/\1/p' "$ROOT/src-tauri/src/installer.rs"); do
  dry_run "$apt_path" "$arg"
  assert_eq "accepts the app's '$arg'" 0 "$CODE"
done
if [ "$(id -u)" -ne 0 ]; then
  out=$(bash "$HELPER" ffmpeg 2>/dev/null)
  assert_eq "a real run needs root" 2 $?
  assert_contains "and says so" "$out" "error Run this as root"
fi
assert_eq "never exits with pkexec's own codes" "" "$(grep -nE '(exit|fail .*) 12[67]\b' "$HELPER")"

echo "== One place for the pinned values"
helper_version=$(sed -n 's/^LACT_VERSION="\(.*\)"$/\1/p' "$HELPER")
core_version=$(sed -n 's/^pub const VERIFIED_VERSION: &str = "\(.*\)";$/\1/p' "$ROOT/src-tauri/crates/redline-core/src/lact.rs")
assert_eq "the helper pins the LACT version redline-core is verified with" "$core_version" "$helper_version"
bad_sha=$(awk 'NF == 3 && $2 ~ /\.deb$/ && $3 !~ /^[0-9a-f]{64}$/' "$HELPER")
assert_eq "every pinned checksum is a SHA256" "" "$bad_sha"
assert_eq "install-deps.sh pins nothing of its own" "" "$(grep -nE 'VERSION=|https?://|sha256' "$ROOT/scripts/install-deps.sh")"
assert_contains "install-deps.sh runs the helper" "$(cat "$ROOT/scripts/install-deps.sh")" "src-tauri/linux/install-dependency"
out=$(PATH="$(make_path wrapper pacman)" bash "$ROOT/scripts/install-deps.sh" --dry-run ffmpeg 2>/dev/null)
assert_contains "install-deps.sh passes --dry-run through" "$out" "run pacman -S --needed --noconfirm ffmpeg"
os_release 'ID=arch'
out=$(PATH="$(make_path wrapper-all pacman)" REDLINE_OS_RELEASE="$TMP/os-release" bash "$ROOT/scripts/install-deps.sh" --dry-run 2>/dev/null)
assert_eq "install-deps.sh skips an unsupported component and goes on" 0 $?
assert_contains "the unsupported LACT is reported" "$out" "error The guided install of LACT needs apt"
assert_contains "the components after it still run" "$out" "run flatpak install -y --noninteractive --system flathub"
assert_contains "all of them" "$out" "run pacman -S --needed --noconfirm ffmpeg"
assert_contains "MangoHud too" "$out" "run pacman -S --needed --noconfirm mangohud"
out=$(PATH="$(make_path wrapper-bad pacman)" bash "$ROOT/scripts/install-deps.sh" --dry-run nonsense ffmpeg 2>/dev/null)
assert_eq "install-deps.sh stops on any other failure" 2 $?
assert_not_contains "without running the rest" "$out" "run pacman"
assert_contains "every real step is bounded" "$(cat "$HELPER")" 'timeout --kill-after=30 "$STEP_TIMEOUT" "$@"' 

echo "== Packaging"
assert_eq "the helper is executable in git" 100755 "$(git -C "$ROOT" ls-files -s src-tauri/linux/install-dependency | awk '{ print $1 }')"
assert_contains "the polkit action runs only the installed helper" "$(cat "$POLICY")" \
  "<annotate key=\"org.freedesktop.policykit.exec.path\">$INSTALLED_AT</annotate>"
assert_contains "the polkit action is com.fabiosm46.redline.install" "$(cat "$POLICY")" '<action id="com.fabiosm46.redline.install">'
assert_contains "the polkit comment lists the new name" "$(cat "$POLICY")" "ffmpeg and mangohud"
assert_contains "the password prompt names MangoHud" "$(cat "$POLICY")" "ffmpeg or MangoHud"
assert_contains "the app runs the installed helper" "$(cat "$ROOT/src-tauri/src/installer.rs")" "pub const HELPER: &str = \"$INSTALLED_AT\";"
for conf in tauri.conf.json tauri.release.linux.conf.json; do
  files=$(jq -c '.bundle.linux.deb.files' "$ROOT/src-tauri/$conf")
  assert_eq "$conf ships the helper" '"linux/install-dependency"' "$(jq -c --arg k "$INSTALLED_AT" '.[$k]' <<<"$files")"
  assert_eq "$conf ships the policy" '"linux/com.fabiosm46.redline.install.policy"' "$(jq -c --arg k "$POLICY_AT" '.[$k]' <<<"$files")"
done

if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck "$HELPER" "$ROOT/scripts/install-deps.sh"; then ok "shellcheck clean"; else bad "shellcheck"; fi
else
  echo "  skip — shellcheck is not installed"
fi

echo
echo "install-dependency: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
