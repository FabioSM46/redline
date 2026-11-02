#!/usr/bin/env bash
# Sourced by the clean install checks, which run inside a container, and by their test, which
# does not. `command -v curl` proves that a tool is on the test system; this proves that the
# package itself declares it, which is all a minimal system has to go on.

# Prints one dependency name per line from what the packaging tool reports on stdin, without
# version constraints: `dpkg-deb -f <file> Depends` (deb), `rpm -q --requires` (fedora) or
# `pacman -Qi` (arch). Of a deb alternative only the first name stays: `wget | curl` can be
# satisfied without curl.
dependency_names() {
  case $1 in
    deb)
      tr '\n,' ' \n' | sed -E 's/[|(].*$//; s/[[:space:]]//g; s/:.*$//'
      ;;
    fedora)
      awk '{ print $1 }'
      ;;
    arch)
      # Continuation lines are indented: pacman wraps long fields when it sees a terminal.
      awk '/^Depends On/ { on = 1; sub(/^[^:]*:/, ""); print; next }
           on && /^[[:space:]]/ { print; next }
           { on = 0 }' |
        tr -s '[:space:]' '\n' | sed -E 's/[<>=].*$//'
      ;;
    *)
      echo "error: unknown package format $1" >&2
      return 2
      ;;
  esac
}

# package_declares <deb|fedora|arch> <name>: true when stdin declares exactly that package, so
# that libcurl4 or python-curl does not stand for curl.
package_declares() {
  local names
  names=$(dependency_names "$1") || return 2
  # Not a pipe: grep -q closing it early would fail the pipeline under pipefail.
  grep -Fxq -- "$2" <<<"$names"
}
