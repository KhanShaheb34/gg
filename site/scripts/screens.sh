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
crop search small-search 900 378 0 0 # stops above the details' parent and author lines (an email address)
crop menu menu-branch 560 370 0 80
crop worktrees worktrees 1280 490 0 40 # the toolbar, the graph and the side panel
crop worktrees-narrow worktrees 300 450 980 80 # the side panel alone, for phones
crop commit small-committed-on-branch 900 520 0 40 # the small window: the Commit tab, Committed on branch
crop commit-narrow small-committed-on-branch 240 480 660 80 # the Commit tab alone, for phones
crop prs small-pr-tab-create 900 520 0 40 # the small window: the Pull request tab's form
crop prs-narrow small-pr-tab-create 240 480 660 80 # the form alone, for phones
crop pr-checks small-pr-tab-details 900 520 0 40 # a pull request's status, checks and comments
crop pr-checks-narrow small-pr-tab-details 240 444 660 80 # down to the end of the first comment
crop notice notice-merge-push 410 164 568 630
crop force dialog-force-push 490 214 395 98
crop conflict small-banner-merge-partly 660 180 0 80
crop log small-log-open 660 135 0 403 # down to the status bar
crop settings settings 830 424 225 37 # down to the line above Tools
crop shortcuts shortcuts 850 674 215 37

# The same graph, its top rows in the light theme and the rest in the dark one.
for scale in 1 2; do
  magick "$shots/${scale}x/graph-branchy-light.png" -crop "$(geometry "$scale" 600 234 0 0)" +repage "$tmp/l.png"
  magick "$shots/${scale}x/graph-branchy-dark.png" -crop "$(geometry "$scale" 600 206 0 234)" +repage "$tmp/d.png"
  magick "$tmp/l.png" "$tmp/d.png" -append "$tmp/themes.png"
  encode "$tmp/themes.png" "$out/themes-${scale}x.webp"
done
