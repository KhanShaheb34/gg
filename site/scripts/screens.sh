#!/usr/bin/env bash
# Crops the app's screenshots (docs/perf/screens) into the WebP images the page uses.
# Needs ImageMagick (magick) and cwebp. Run from anywhere: bash site/scripts/screens.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
src="$root/docs/perf/screens"
out="$root/site/public/img"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out"

# crop <name> <screenshot> <geometry>: one WebP per theme.
crop() {
  for theme in light dark; do
    magick "$src/$2-$theme.png" -crop "$3" +repage "$tmp/$1-$theme.png"
    cwebp -quiet -q 90 -m 6 "$tmp/$1-$theme.png" -o "$out/$1-$theme.webp"
  done
}

crop graph graph-branchy 1280x800+0+0
crop graph-narrow graph-branchy 600x640+0+0
crop search small-search 900x440+0+0
crop menu menu-branch 560x320+0+80
crop notice notice-merge-push 410x164+870+636
crop force dialog-force-push 490x214+395+98
crop conflict small-banner-merge-partly 900x146+0+80
crop log small-log-open 900x144+0+416

# The same graph, its top half in the light theme and the rest in the dark one.
magick "$src/graph-branchy-light.png" -crop 600x234+0+0 +repage "$tmp/l.png"
magick "$src/graph-branchy-dark.png" -crop 600x206+0+234 +repage "$tmp/d.png"
magick "$tmp/l.png" "$tmp/d.png" -append "$tmp/themes.png"
cwebp -quiet -q 90 -m 6 "$tmp/themes.png" -o "$out/themes.webp"
