#!/bin/sh
# Builds gg for macOS as one universal app (Apple silicon and Intel), signs it ad hoc and
# packs it in a disk image with a link to /Applications (made by hdiutil alone: no Finder
# window styling through AppleScript). Writes into dist-release/ (git-ignored):
#   gg-macos-universal.dmg
#   gg-macos-universal.dmg.sha256   ("<sha256>  gg-macos-universal.dmg", as shasum -c reads)
#   latest.json                      ({"version","file","sizeMb","sha256"} on one line, the
#                                     shape the site's latest.json has)
# Needs the Rust targets aarch64-apple-darwin and x86_64-apple-darwin, pnpm and the UI's
# packages. The version is the workspace Cargo.toml's.
set -eu
cd "$(dirname "$0")/.."

fail() {
  echo "release-macos: $*" >&2
  exit 1
}

[ "$(uname -s)" = Darwin ] || fail "builds on macOS only"

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)
[ -n "$version" ] || fail "no version in Cargo.toml's [workspace.package]"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  libdir=$(rustc --print target-libdir --target "$target" 2>/dev/null) || libdir=
  [ -n "$libdir" ] && [ -d "$libdir" ] && ls "$libdir"/libstd-* >/dev/null 2>&1 ||
    fail "the Rust target $target is not installed for $(rustc --version) (rustup target add $target)"
done

pnpm tauri build --target universal-apple-darwin --bundles app

app=target/universal-apple-darwin/release/bundle/macos/gg.app
[ -d "$app" ] || fail "$app was not built"
exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
archs=$(lipo -archs "$app/Contents/MacOS/$exe")
for arch in arm64 x86_64; do
  case " $archs " in
  *" $arch "*) ;;
  *) fail "$app/Contents/MacOS/$exe has [$archs], not $arch" ;;
  esac
done
bundled=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
[ "$bundled" = "$version" ] || fail "the app says version $bundled, Cargo.toml $version"

# Ad hoc: no Developer ID, so Gatekeeper asks before the first launch.
codesign --force --deep -s - "$app"
codesign --verify --deep --strict "$app"

out=dist-release
file=gg-macos-universal.dmg
mkdir -p "$out"
rm -f "$out/$file" "$out/$file.sha256" "$out/latest.json"

stage=$(mktemp -d "${TMPDIR:-/tmp}/gg-dmg.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname gg -srcfolder "$stage" -ov -format UDZO "$out/$file" >/dev/null

# A pipe hides a failed shasum (sh has no pipefail): the hash is checked for its shape.
sha=$(shasum -a 256 "$out/$file" | cut -d' ' -f1)
printf '%s' "$sha" | grep -Eq '^[0-9a-f]{64}$' || fail "could not compute the sha256 of $out/$file"
printf '%s  %s\n' "$sha" "$file" >"$out/$file.sha256"
bytes=$(stat -f %z "$out/$file")
# Megabytes as Finder counts them (10^6 bytes), one decimal.
size=$(awk -v b="$bytes" 'BEGIN { printf "%.1f", b / 1000000 }')
printf '{"version":"%s","file":"%s","sizeMb":%s,"sha256":"%s"}\n' \
  "$version" "$file" "$size" "$sha" >"$out/latest.json"

echo "release-macos: gg $version, $archs, $size MB"
echo "  $out/$file"
echo "  $out/$file.sha256"
echo "  $out/latest.json"
