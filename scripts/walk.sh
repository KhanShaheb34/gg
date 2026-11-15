#!/usr/bin/env bash
# Drives the walk (src/lib/perf/walkProbe.ts) in the release app built with the `perf`
# feature (scripts/perf.sh builds it): makes a throwaway repository (scripts/walk-fixture.sh)
# under <outdir>, launches the app on it with its own config folder (GG_CONFIG_DIR), answers
# the walk's `walk-need` marks by changing files, screenshots the window at `walk-shot` marks
# into <outdir>, and prints every mark when the walk is done (or after 7 minutes).
#
#   scripts/walk.sh <outdir>
#
# macOS only. Keep the screen unlocked and the window uncovered: a covered window gets no
# animation frames and the walk stalls; the marks then say how far it got. Screenshots need
# the Screen Recording permission for the terminal; without it they are left out.
set -uo pipefail
out=$(mkdir -p "$1" && cd "$1" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
bundle=$root/target/release/bundle/macos/gg.app
app=$bundle/Contents/MacOS/gg-desktop
[ -x "$app" ] || { echo "no app at $app: build it with the perf feature first" >&2; exit 1; }
if pgrep -f "/gg.app/Contents/MacOS/gg-desktop" > /dev/null; then
  echo "gg is running; quit every copy of it first" >&2
  exit 1
fi
# The window id for screenshots: a small helper, compiled once into <outdir>.
winid=$out/.winid
[ -x "$winid" ] || swiftc -O -o "$winid" "$root/scripts/winid.swift" 2> /dev/null || winid=""
work=$("$root/scripts/walk-fixture.sh" "$out")
W=$(dirname "$work")

log=$W/perf.log
open -n -a "$bundle" --env GG_OPEN="$work" --env GG_PERF_LOG="$log" --env GG_PERF_SCROLL=walk \
  --env GG_CONFIG_DIR="$W/config"
sleep 1
pid=$(pgrep -n -f "^$app")
seen=0
deadline=$(( $(date +%s) + 420 ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  [ -f "$log" ] || { sleep 0.2; continue; }
  n=$(wc -l < "$log")
  if [ "$n" -gt "$seen" ]; then
    tail -n +"$((seen + 1))" "$log" | head -n "$((n - seen))" | while IFS=$'\t' read -r name _ _; do
      case $name in
        "walk-need dirty") echo "dirty $(date +%s%N)" >> "$work/notes.txt" ;;
        "walk-need resolve app.txt") echo "resolved" > "$work/app.txt" ;;
        walk-shot\ *)
          id=""
          [ -n "$winid" ] && id=$("$winid" "$pid")
          [ -n "$id" ] && screencapture -x -o -l "$id" "$out/${name#walk-shot }.png"
          ;;
      esac
    done
    seen=$n
  fi
  grep -q "^scroll-done	" "$log" && break
  sleep 0.2
done
grep -E "^(first-rows|fully-loaded|walk|page-error)" "$log" | cut -f1,2
kill "$pid" 2>/dev/null
sleep 3
echo "repo: $W"
