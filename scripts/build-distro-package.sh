#!/usr/bin/env bash
# Builds the Arch or Fedora package of this checkout, inside a throwaway container of that
# system, and copies it to <out>. The release job runs it once per system:
#
#   docker run --rm -v "$PWD:/repo:ro" -v "$PWD/packages:/out" archlinux:latest \
#     bash /repo/scripts/build-distro-package.sh arch /repo /out
#
# As with the .deb, the package comes from the tagged tree: the source archive is made here.

set -euo pipefail

[ "$#" -eq 3 ] || { echo "usage: $0 arch|fedora <repo> <out>" >&2; exit 2; }
system=$1 repo=$2 out=$3

if [ ! -f /.dockerenv ] && [ ! -f /run/.containerenv ]; then
  echo "error: this script installs packages and adds a user; run it in a container" >&2
  exit 2
fi

version=$(sed -n 's/^  "version": "\(.*\)",$/\1/p' "$repo/package.json")
archive() {
  tar -C "$repo" --exclude=node_modules --exclude=target --exclude=.git --exclude=dist \
    --exclude=packages --transform "s,^\.,redline-$version," -czf "$1" .
}

case $system in
  arch)
    pacman -Syu --noconfirm --needed base-devel
    useradd -m builder
    echo 'builder ALL=(ALL) NOPASSWD: ALL' >/etc/sudoers.d/builder
    work=/home/builder/package
    mkdir -p "$work"
    cp "$repo/packaging/arch/PKGBUILD" "$work/"
    archive "$work/redline-$version.tar.gz"
    chown -R builder "$work"
    su builder -c "cd $work && makepkg --syncdeps --noconfirm"
    cp "$work"/redline-"$version"-*.pkg.tar.zst "$out/"
    ;;
  fedora)
    dnf install -y rpm-build dnf-plugins-core
    mkdir -p /root/rpmbuild/SOURCES
    archive "/root/rpmbuild/SOURCES/redline-$version.tar.gz"
    dnf builddep -y "$repo/packaging/fedora/redline.spec"
    rpmbuild -bb "$repo/packaging/fedora/redline.spec"
    cp /root/rpmbuild/RPMS/x86_64/redline-"$version"-*.rpm "$out/"
    ;;
  *)
    echo "usage: $0 arch|fedora <repo> <out>" >&2
    exit 2
    ;;
esac
