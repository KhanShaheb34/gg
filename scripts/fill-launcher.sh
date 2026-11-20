#!/bin/sh
# Writes the gg launcher (src-tauri/resources/gg) filled in for the macOS app bundle into
# <file>, mode 755: the version, and no program path (on macOS the launcher finds the app by
# its bundle identifier). The same bytes as gg_core::cli::launcher_script(version, None); a
# test beside it (the_release_fill_matches_launcher_script) runs this script and compares.
#
#   scripts/fill-launcher.sh <version> <file>
#
# release-macos.sh puts it in gg.app/Contents/Resources/gg, which a Homebrew cask links
# into its bin. Fails, leaving no <file>, on a version other than letters, digits, ".", "+"
# and "-", or if any placeholder is left.
set -eu

fail() {
  echo "fill-launcher: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || fail "usage: fill-launcher.sh <version> <file>"
version=$1
file=$2
case $version in
'' | *[!0-9A-Za-z.+-]*) fail "not a plain version: '$version'" ;;
esac

template=$(dirname "$0")/../src-tauri/resources/gg
[ -f "$template" ] || fail "no launcher template at $template"

tmp=$file.fill.$$
trap 'rm -f "$tmp"' EXIT
rm -f "$tmp"
# The version is plain (checked above), so it cannot be read as sed syntax.
LC_ALL=C sed -e "s/@GG_VERSION@/$version/g" -e "s/@GG_APP@/''/g" "$template" >"$tmp"
if grep -q '@GG_' "$tmp"; then
  fail "a placeholder is left in the launcher"
fi
grep -Fqx "# gg launcher, installed by gg $version" "$tmp" || fail "the launcher has no marker line"
chmod 755 "$tmp"
mv -f "$tmp" "$file"
trap - EXIT
