#!/usr/bin/env bash
# The page's screenshots, in one command: captures the app at 1× and 2× device pixels
# (scripts/shots.mjs, Playwright + gg-devserver), then crops each shot into a 1× and a 2× WebP
# in public/img/. Needs ImageMagick (magick) and cwebp; the capture needs pnpm install and cargo.
#   bash site/scripts/screens.sh               # capture and crop
#   bash site/scripts/screens.sh --no-capture  # crop the shots already in site/.shots/
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
shots="$root/site/.shots"
out="$root/site/public/img"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$out"

if [[ "${1:-}" != "--no-capture" ]]; then
  node "$root/site/scripts/shots.mjs"
fi

# UI screenshots are flat colour and sharp edges: lossless WebP keeps every text edge as
# captured and stays small for them.
encode() { cwebp -quiet -lossless -z 9 -metadata none "$1" -o "$2"; }

# geometry <scale> <W> <H> <X> <Y>: a crop given in CSS pixels, in device pixels.
geometry() { echo "$(($2 * $1))x$(($3 * $1))+$(($4 * $1))+$(($5 * $1))"; }

# crop <name> <shot> <W> <H> <X> <Y>: public/img/<name>-<theme>-<scale>x.webp for both
# themes and scales. Coordinates are CSS pixels of the 1280×800 or 900×560 window.
crop() {
  local name=$1 shot=$2
  shift 2
  for scale in 1 2; do
    for theme in light dark; do
      magick "$shots/${scale}x/$shot-$theme.png" -crop "$(geometry "$scale" "$@")" +repage "$tmp/c.png"
      encode "$tmp/c.png" "$out/$name-$theme-${scale}x.webp"
    done
  done
}

crop graph graph-branchy 1280 800 0 0
crop graph-narrow graph-branchy 600 640 0 0
crop search small-search 900 400 0 0 # stops above the details' author line (an email address)
crop menu menu-branch 560 320 0 80
crop notice notice-merge-push 410 164 870 636
crop force dialog-force-push 490 214 395 98
crop conflict small-banner-merge-partly 900 146 0 80
crop log small-log-open 900 144 0 416

# The same graph, its top rows in the light theme and the rest in the dark one.
for scale in 1 2; do
  magick "$shots/${scale}x/graph-branchy-light.png" -crop "$(geometry "$scale" 600 234 0 0)" +repage "$tmp/l.png"
  magick "$shots/${scale}x/graph-branchy-dark.png" -crop "$(geometry "$scale" 600 206 0 234)" +repage "$tmp/d.png"
  magick "$tmp/l.png" "$tmp/d.png" -append "$tmp/themes.png"
  encode "$tmp/themes.png" "$out/themes-${scale}x.webp"
done
