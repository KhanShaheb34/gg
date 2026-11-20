#!/bin/sh
# Packs a macOS app bundle as gg's update for macOS: a gzipped tar with <name>.app at its
# top, which Tauri's updater extracts in place of the running app (it drops that first path
# component). No AppleDouble files, extended attributes, ACLs or file flags go in: extracted,
# they would be extra files in the bundle or change it, and break its code signature.
#
#   scripts/update-archive.sh <path/to/gg.app> <out.app.tar.gz>
#
# Then checks the archive as the app will use it: extracted into a temp folder it holds the
# bundle and nothing else, the same files as the bundle, the bundle's code signature
# verifies (codesign --verify --deep --strict), and the gg launcher is in it, executable.
# macOS only.
set -eu

fail() {
  echo "update-archive: $*" >&2
  exit 1
}

[ $# -eq 2 ] || fail "usage: scripts/update-archive.sh <app> <out.app.tar.gz>"
[ "$(uname -s)" = Darwin ] || fail "macOS only"
app=$1
out=$2
[ -d "$app" ] || fail "$app is not a folder"
case $app in *.app | *.app/) ;; *) fail "$app is not an app bundle" ;; esac
codesign --verify --deep --strict "$app" || fail "$app's code signature does not verify"
dir=$(cd "$(dirname "$app")" && pwd)
name=$(basename "$app")
mkdir -p "$(dirname "$out")"
rm -f "$out"
COPYFILE_DISABLE=1 tar --no-xattrs --no-acls --no-fflags --no-mac-metadata \
  -czf "$out" -C "$dir" "$name"

check=$(mktemp -d "${TMPDIR:-/tmp}/gg-update-check.XXXXXX")
trap 'rm -rf "$check"' EXIT
tar -xzf "$out" -C "$check"
[ "$(ls -A "$check")" = "$name" ] || fail "$out holds $(ls -A "$check" | tr '\n' ' '), not just $name"
(cd "$dir" && find "$name" | LC_ALL=C sort) >"$check/.want"
(cd "$check" && find "$name" | LC_ALL=C sort) >"$check/.have"
cmp -s "$check/.want" "$check/.have" || {
  diff "$check/.want" "$check/.have" >&2 || true
  fail "$out does not hold the same files as $app"
}
codesign --verify --deep --strict "$check/$name" || fail "the app extracted from $out does not verify"
[ -x "$check/$name/Contents/Resources/gg" ] || fail "the app extracted from $out has no gg launcher"
echo "update-archive: $out ($(du -h "$out" | cut -f1 | tr -d ' ')): $name, its signature verified once extracted"
