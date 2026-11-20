#!/bin/sh
# Builds gg for macOS as one universal app (Apple silicon and Intel), puts the gg command's
# launcher in it (Contents/Resources/gg, made by fill-launcher.sh), signs it ad hoc and
# packs it in a disk image with a link to /Applications (made by hdiutil alone: no Finder
# window styling through AppleScript), and packs the same signed app as the update the app
# installs when it updates itself (scripts/update-archive.sh, which checks it extracts with
# its signature intact). Writes into dist-release/ (git-ignored):
#   gg-macos-universal.dmg
#   gg-macos-universal.dmg.sha256   ("<sha256>  gg-macos-universal.dmg", as shasum -c reads)
#   gg-macos-universal.app.tar.gz   the update (signed for the updater by the release
#                                     workflow's next step, scripts/sign-update.sh)
#   gg-macos-universal.app.tar.gz.sha256
#   site-latest.json                 ({"version","file","sizeMb","sha256"} on one line, the
#                                     shape the site's latest.json has; not the updater's
#                                     latest.json, which the release workflow writes)
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

# The gg command, inside the app before it is signed, for a Homebrew cask to link into its
# bin (binary "#{appdir}/gg.app/Contents/Resources/gg"): the launcher filled in for this
# version, finding the app by its bundle identifier.
launcher=Contents/Resources/gg
[ -d "$app/Contents/Resources" ] || fail "$app has no Contents/Resources"
sh scripts/fill-launcher.sh "$version" "$app/$launcher"

# Checks the launcher in app bundle $1: a plain executable file, mode 755, every placeholder
# filled, saying this version. (Its variables are named apart from the script's own: sh has
# no local ones.)
check_launcher() {
  cl_file=$1/$launcher
  [ -f "$cl_file" ] && [ ! -L "$cl_file" ] || fail "$cl_file is missing (or not a plain file)"
  [ -x "$cl_file" ] || fail "$cl_file is not executable"
  cl_mode=$(stat -f %Lp "$cl_file")
  [ "$cl_mode" = 755 ] || fail "$cl_file has mode $cl_mode, not 755"
  if grep -q '@GG_' "$cl_file"; then
    fail "$cl_file still has a placeholder"
  fi
  cl_said=$("$cl_file" --version) || fail "$cl_file --version failed"
  [ "$cl_said" = "gg $version" ] || fail "$cl_file --version says '$cl_said', not 'gg $version'"
}
check_launcher "$app"

# Ad hoc: no Developer ID, so Gatekeeper asks before the first launch.
codesign --force --deep -s - "$app"
codesign --verify --deep --strict "$app"
check_launcher "$app"

out=dist-release
file=gg-macos-universal.dmg
mkdir -p "$out"
update=gg-macos-universal.app.tar.gz
rm -f "$out/$file" "$out/$file.sha256" "$out/site-latest.json" "$out/latest.json" \
  "$out/$update" "$out/$update.sha256" "$out/$update.sig"

stage=$(mktemp -d "${TMPDIR:-/tmp}/gg-dmg.XXXXXX")
trap 'rm -rf "$stage"' EXIT
cp -R "$app" "$stage/"
# The copy that goes in the disk image: still signed, still carrying the launcher.
codesign --verify --deep --strict "$stage/gg.app"
check_launcher "$stage/gg.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname gg -srcfolder "$stage" -ov -format UDZO "$out/$file" >/dev/null

# The update: the very app that went in the disk image (signed, with the launcher), checked
# once extracted.
sh scripts/update-archive.sh "$app" "$out/$update"

# The sha256 of $out/$1 into $out/$1.sha256, as shasum -c reads it. A pipe hides a failed
# shasum (sh has no pipefail): the hash is checked for its shape.
checksum() {
  cs_sha=$(shasum -a 256 "$out/$1" | cut -d' ' -f1)
  printf '%s' "$cs_sha" | grep -Eq '^[0-9a-f]{64}$' || fail "could not compute the sha256 of $out/$1"
  printf '%s  %s\n' "$cs_sha" "$1" >"$out/$1.sha256"
}
checksum "$file"
checksum "$update"
sha=$(cut -d' ' -f1 <"$out/$file.sha256")
bytes=$(stat -f %z "$out/$file")
# Megabytes as Finder counts them (10^6 bytes), one decimal.
size=$(awk -v b="$bytes" 'BEGIN { printf "%.1f", b / 1000000 }')
printf '{"version":"%s","file":"%s","sizeMb":%s,"sha256":"%s"}\n' \
  "$version" "$file" "$size" "$sha" >"$out/site-latest.json"

echo "release-macos: gg $version, $archs, $size MB"
echo "  $out/$file"
echo "  $out/$file.sha256"
echo "  $out/$update (to be signed: scripts/sign-update.sh $version $out/$update)"
echo "  $out/$update.sha256"
echo "  $out/site-latest.json"
