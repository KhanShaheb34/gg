#!/usr/bin/env bash
# Drives a walk in the release app built with the `perf` feature (scripts/perf.sh builds it):
# makes a throwaway repository (scripts/walk-fixture.sh) under <outdir>, launches the app on it
# with its own config folder (GG_CONFIG_DIR), answers the walk's `walk-need` marks by changing
# files (or by running `gg`), screenshots the window at `walk-shot` marks into <outdir>, and
# prints every mark when the walk is done (or after 7 minutes).
#
#   scripts/walk.sh [--mode walk | walk3 | walk3-dark] [--size WxH] [--via-gg] <outdir>
#
# --mode walk (default): plan 2's walk (src/lib/perf/walkProbe.ts). walk3, walk3-dark: plan
#   3's (worktreeWalkProbe.ts): settings, the staging routine, worktrees, the commit panel, a
#   pull request and `gg` into the running app, in the light or the dark theme (set through
#   the app's settings). Its repository's `origin` is a github.com URL that git sends to a bare
#   repository beside it; `gh` is a recording fake.
# --size: the window's size in logical pixels (GG_PERF_WINDOW), e.g. 900x560.
# --via-gg: the app is started by the `gg` command (the launcher the app installs, filled in
#   here, pointed at this build with GG_APP_BIN) while it is not running, instead of `open`.
#
# Every run's programs come from <outdir>/…/tools first: the app is given a stand-in login
# shell whose PATH is that folder, /usr/bin and /bin (SHELL), so the real `gh` (and anything
# else the user's profile adds) is never found; the fake `gh` answers from files the driver
# writes. Editors, terminals and the browser are never started: the walk does not use them.
#
# macOS only. Keep the screen unlocked and the window uncovered: a covered window gets no
# animation frames and the walk stalls; the marks then say how far it got. Screenshots need
# the Screen Recording permission for the terminal; without it they are left out.
set -uo pipefail
mode=walk
size=""
via_gg=0
while [ $# -gt 1 ]; do
  case $1 in
    --mode) mode=$2; shift 2 ;;
    --size) size=$2; shift 2 ;;
    --via-gg) via_gg=1; shift ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
case $mode in walk | walk3 | walk3-dark) ;; *) echo "unknown mode $mode" >&2; exit 2 ;; esac
out=$(mkdir -p "$1" && cd "$1" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
bundle=$root/target/release/bundle/macos/gg.app
app=$bundle/Contents/MacOS/gg-desktop
[ -x "$app" ] || { echo "no app at $app: build it with the perf feature first" >&2; exit 1; }
if pgrep -f "/Contents/MacOS/gg-desktop" > /dev/null; then
  echo "gg is running; quit every copy of it first" >&2
  exit 1
fi
# The window id for screenshots: a small helper, compiled once into <outdir>.
winid=$out/.winid
[ -x "$winid" ] || swiftc -O -o "$winid" "$root/scripts/winid.swift" 2> /dev/null || winid=""
if [ "$mode" = walk ]; then
  work=$("$root/scripts/walk-fixture.sh" "$out")
else
  work=$("$root/scripts/walk-fixture.sh" --github "$out")
fi
W=$(dirname "$work")

# The stand-in login shell and the fake gh (as e2e/fixtures.ts has them).
tools=$W/tools
mkdir -p "$tools" "$W/gh/calls" "$W/gh/replies"
printf '#!/bin/sh\nPATH=%s:/usr/bin:/bin\nexport PATH\neval "$2"\n' "$tools" > "$tools/fake-shell"
cat > "$tools/gh" <<'EOF'
#!/bin/sh
PATH=/usr/bin:/bin
export PATH
d="$(cd "$(dirname "$0")/.." && pwd)/gh"
i=1
while ! mkdir "$d/calls/$i" 2>/dev/null; do i=$((i+1)); done
for a in "$@"; do printf '%s\0' "$a"; done > "$d/calls/$i/argv"
cat > "$d/calls/$i/stdin"
key="$1-$2"
case " $* " in *" --head "*) if [ "$key" = pr-list ]; then key=pr-list-head; fi;; esac
r="$d/replies/$key"
if [ -f "$r.err" ]; then cat "$r.err" >&2; fi
if [ -f "$r.out" ]; then cat "$r.out"; fi
if [ -f "$r.code" ]; then exit "$(cat "$r.code")"; fi
exit 0
EOF
chmod +x "$tools/fake-shell" "$tools/gh"
echo '[]' > "$W/gh/replies/pr-list.out"
echo '[]' > "$W/gh/replies/pr-list-head.out"
echo '{"defaultBranchRef":{"name":"main"}}' > "$W/gh/replies/repo-view.out"
echo 'https://github.com/octo/cat/pull/21' > "$W/gh/replies/pr-create.out"
# The `gg` command as the app installs it, pointed at this build.
sed -e "s|@GG_VERSION@|test|" -e "s|@GG_APP@|''|" "$root/src-tauri/resources/gg" > "$tools/gg"
chmod +x "$tools/gg"
run_env=(GG_PERF_LOG="$W/perf.log" GG_PERF_SCROLL="$mode" GG_CONFIG_DIR="$W/config"
  SHELL="$tools/fake-shell" PATH="$tools:/usr/bin:/bin")
[ -n "$size" ] && run_env+=(GG_PERF_WINDOW="$size")

log=$W/perf.log
if [ "$via_gg" = 1 ]; then
  (cd "$work" && env "${run_env[@]}" GG_APP_BIN="$app" "$tools/gg") || { echo "gg failed" >&2; exit 1; }
  echo "started by gg: $(date +%s)"
else
  env_args=()
  for e in "${run_env[@]}"; do env_args+=(--env "$e"); done
  open -n -a "$bundle" --env GG_OPEN="$work" "${env_args[@]}"
fi
sleep 1
pid=$(pgrep -n -f "^$app")
[ -n "$pid" ] || { echo "the app did not start" >&2; exit 1; }
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
        "walk-need edit worktree walk-wt")
          wt=$W/work.worktrees/walk-wt
          echo "changed in the worktree" >> "$wt/app.txt"
          echo "new" > "$wt/new.txt"
          echo "scratch" > "$wt/scratch.txt"
          ;;
        "walk-need list pr 21")
          printf '[{"number":21,"title":"Walk: a change in a worktree","url":"https://github.com/octo/cat/pull/21","isDraft":false,"headRefName":"walk/wt","baseRefName":"main","headRepositoryOwner":{"id":"X","login":"octo"},"isCrossRepository":false}]\n' \
            > "$W/gh/replies/pr-list.out"
          ;;
        "walk-need gg other")
          # A second launch hands the folder to the running app and exits.
          (cd "$W" && env PATH="$tools:/usr/bin:/bin" GG_APP_BIN="$app" "$tools/gg" other) ||
            echo "gg other failed" >&2
          ;;
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
echo "gh calls:"
for c in "$W"/gh/calls/*/argv; do [ -f "$c" ] && tr '\0' ' ' < "$c" && echo; done
kill "$pid" 2>/dev/null
sleep 3
pgrep -f "^$app" > /dev/null && echo "warning: the app is still running" >&2
echo "repo: $W"
