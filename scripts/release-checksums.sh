#!/usr/bin/env bash
# Writes and checks the SHA256SUMS file a release publishes next to its packages (#157).
#
# The file is in the format `sha256sum -c` reads, one line per package, names without a folder,
# so someone who downloaded a package and SHA256SUMS into the same folder can run
# `sha256sum -c --ignore-missing SHA256SUMS`. It proves that a download is the file on the
# release page, not who built it: the packages and this file are not signed.
#
#   bash scripts/release-checksums.sh write <folder>
#       Writes <folder>/SHA256SUMS over every file in <folder>, then checks it.
#   bash scripts/release-checksums.sh check <folder>
#       Fails unless SHA256SUMS lists exactly the other files of <folder>, each once, and
#       every checksum matches.
#   bash scripts/release-checksums.sh assets <folder>
#       Reads asset names from stdin, one per line, and fails unless they are exactly
#       SHA256SUMS plus the files it lists: a draft that kept an asset from an earlier run
#       would otherwise publish a package the file does not cover.

set -euo pipefail
export LC_ALL=C

SUMS=SHA256SUMS

die() { echo "error: $*" >&2; exit 1; }

usage() {
  echo "usage: release-checksums.sh write|check|assets <folder>" >&2
  exit 2
}

# The packages of <folder>: every entry except SHA256SUMS, sorted. Anything that is not a plain
# file with a plain name is refused, so a name never needs the escaping `sha256sum` applies.
# `~` is refused too: a release stores an asset under a rewritten name when its file name has
# characters outside this set, and SHA256SUMS would then list a name nobody can download.
packages() {
  local entry name
  for entry in "$1"/* "$1"/.[!.]*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    name=${entry##*/}
    [ "$name" != "$SUMS" ] || continue
    [ -f "$entry" ] && [ ! -L "$entry" ] || die "$name is not a regular file"
    [[ "$name" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]] || die "unexpected package name: $name"
    printf '%s\n' "$name"
  done | sort
}

# The names SHA256SUMS lists, in the order it lists them. A line that is not
# `<64 hex digits>  <name>` is refused.
listed() {
  local line
  while IFS= read -r line || [ -n "$line" ]; do
    [[ "$line" =~ ^[0-9a-f]{64}\ \ ([A-Za-z0-9][A-Za-z0-9._+-]*)$ ]] \
      || die "$SUMS has a line that is not '<sha256>  <name>'"
    printf '%s\n' "${BASH_REMATCH[1]}"
  done <"$1/$SUMS"
}

# Fails, naming the difference, unless the two sorted lists are the same.
same_names() { # <what the first list is> <first> <what the second is> <second>
  local only_first only_second
  only_first=$(comm -23 <(printf '%s\n' "$2") <(printf '%s\n' "$4"))
  only_second=$(comm -13 <(printf '%s\n' "$2") <(printf '%s\n' "$4"))
  [ -z "$only_first" ] || echo "error: only in $1: $(paste -sd ' ' <<<"$only_first")" >&2
  [ -z "$only_second" ] || echo "error: only in $3: $(paste -sd ' ' <<<"$only_second")" >&2
  [ -z "$only_first" ] && [ -z "$only_second" ]
}

check() {
  local dir=$1 present names duplicates
  [ -f "$dir/$SUMS" ] && [ ! -L "$dir/$SUMS" ] || die "$dir/$SUMS is missing"
  present=$(packages "$dir")
  [ -n "$present" ] || die "$dir holds no package"
  names=$(listed "$dir")
  duplicates=$(sort <<<"$names" | uniq -d)
  [ -z "$duplicates" ] || die "$SUMS lists more than once: $(paste -sd ' ' <<<"$duplicates")"
  same_names "the folder" "$present" "$SUMS" "$(sort <<<"$names")" \
    || die "$SUMS does not list exactly the packages of $dir"
  (cd "$dir" && sha256sum --check --strict --quiet "$SUMS") \
    || die "a checksum of $SUMS does not match its package"
  echo "$SUMS covers $(wc -l <<<"$present") package(s):"
  cat "$dir/$SUMS"
}

write() {
  local dir=$1 present
  present=$(packages "$dir")
  [ -n "$present" ] || die "$dir holds no package"
  rm -f "$dir/$SUMS"
  (
    cd "$dir"
    mapfile -t files <<<"$present"
    sha256sum -- "${files[@]}" >"$SUMS.tmp"
    mv "$SUMS.tmp" "$SUMS"
  )
  check "$dir"
}

assets() {
  local dir=$1 published expected
  published=$(grep -v '^$' | sort || true)
  [ -f "$dir/$SUMS" ] || die "$dir/$SUMS is missing"
  expected=$({ listed "$dir"; echo "$SUMS"; } | sort)
  same_names "the release" "$published" "$SUMS and the packages it lists" "$expected" \
    || die "the release assets are not exactly $SUMS and the packages it lists;" \
      "delete from the draft what is only in the release, then run the job again"
  echo "the release holds exactly $SUMS and the packages it lists"
}

[ "$#" -eq 2 ] || usage
[ -d "$2" ] || die "$2 is not a folder"
case "$1" in
  write) write "$2" ;;
  check) check "$2" ;;
  assets) assets "$2" ;;
  *) usage ;;
esac
