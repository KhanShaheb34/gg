#!/usr/bin/env bash
# Drives a walk in the release app built with the `perf` feature (scripts/perf.sh builds it):
# makes a throwaway repository (scripts/walk-fixture.sh) under <outdir>, launches the app on it
# with its own config folder (GG_CONFIG_DIR), answers the walk's `walk-need` marks by changing
# files (or by running `gg`), screenshots the window at `walk-shot` marks into <outdir>, and
# prints every mark when the walk is done (or after 12 minutes).
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
# writes, and `ssh` and `open` there fail loudly (and say so in <repo>/blocked.log), so no
# network host is reached even by a git that would use ssh. git, in the fixture and in the
# app, runs without the user's global and system config (GIT_CONFIG_GLOBAL is an empty file,
# GIT_CONFIG_NOSYSTEM=1), and origin has an explicit local push URL. Editors, terminals and
# the browser are never started: the perf build records them (GG_PERF_LAUNCHES) instead.
#
# The app is the perf build (scripts/perf.sh builds it) under its own bundle identifier,
# com.shakirul.gg.perf: it shares no WebKit storage, single-instance identity or
# LaunchServices registration with an installed gg (com.shakirul.gg), which the walk never
# starts, reads or changes. `gg` into the running app goes through the real launcher path
# (`open -n -b com.shakirul.gg.perf`); --via-gg (not running) uses its GG_APP_BIN hook, since
# a LaunchServices launch gets none of the run's environment.
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
perf_id=com.shakirul.gg.perf
[ -x "$app" ] || { echo "no app at $app: build it with scripts/perf.sh first" >&2; exit 1; }
id=$(/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$bundle/Contents/Info.plist" 2> /dev/null)
if [ "$id" != "$perf_id" ]; then
  echo "the app at $bundle is $id, not the perf build ($perf_id): build it with scripts/perf.sh" >&2
  exit 1
fi
# Only another copy of this perf build stops the run: an installed gg (com.shakirul.gg) may
# run beside it (P3-R34) and is never signalled, focused or otherwise touched; the driver
# kills only the pid it started.
if pgrep -f "^$app" > /dev/null; then
  echo "the perf build at $app is running; quit it first" >&2
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
# git without the user's global and system config, as the fixture was made.
export GIT_CONFIG_GLOBAL="$W/gitconfig-empty" GIT_CONFIG_NOSYSTEM=1
unset GIT_CONFIG_SYSTEM GIT_CONFIG_COUNT GIT_CONFIG_PARAMETERS

# The stand-in login shell and the fake gh (as e2e/fixtures.ts has them).
tools=$W/tools
mkdir -p "$tools" "$W/gh/calls" "$W/gh/replies"
# ssh and open must never run: they fail, and say so.
for blocked in ssh open; do
  printf '#!/bin/sh
echo "%s $*" >> "%s/blocked.log"
echo "walk: %s must not run here" >&2
exit 97
' \
    "$blocked" "$W" "$blocked" > "$tools/$blocked"
  chmod +x "$tools/$blocked"
done
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
# The `gg` command as the app installs it, finding this build by its own bundle identifier.
sed -e "s|@GG_VERSION@|test|" -e "s|@GG_APP@|''|" \
  -e "s|^GG_BUNDLE_ID='com.shakirul.gg'\$|GG_BUNDLE_ID='$perf_id'|" \
  "$root/src-tauri/resources/gg" > "$W/gg"
chmod +x "$W/gg"
grep -q "^GG_BUNDLE_ID='$perf_id'\$" "$W/gg" || { echo "could not point gg at $perf_id" >&2; exit 1; }
run_env=(GG_PERF_LOG="$W/perf.log" GG_PERF_SCROLL="$mode" GG_CONFIG_DIR="$W/config"
  GG_PERF_LAUNCHES="$W/launches.jsonl" SHELL="$tools/fake-shell" PATH="$tools:/usr/bin:/bin"
  GIT_CONFIG_GLOBAL="$GIT_CONFIG_GLOBAL" GIT_CONFIG_NOSYSTEM=1)
[ -n "$size" ] && run_env+=(GG_PERF_WINDOW="$size")

log=$W/perf.log
pid=""
# Whatever ends the driver (done, Ctrl-C, the deadline), the app it started is quit.
cleanup() {
  [ -n "$pid" ] || return 0
  kill "$pid" 2> /dev/null
  for _ in $(seq 1 50); do kill -0 "$pid" 2> /dev/null || break; sleep 0.1; done
  if kill -0 "$pid" 2> /dev/null; then kill -9 "$pid" 2> /dev/null; fi
  pid=""
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP
if [ "$via_gg" = 1 ]; then
  (cd "$work" && env "${run_env[@]}" GG_APP_BIN="$app" "$W/gg") || { echo "gg failed" >&2; exit 1; }
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
deadline=$(( $(date +%s) + 720 ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  [ -f "$log" ] || { sleep 0.2; continue; }
  n=$(wc -l < "$log")
  if [ "$n" -gt "$seen" ]; then
    tail -n +"$((seen + 1))" "$log" | head -n "$((n - seen))" | while IFS=$'\t' read -r name _ _; do
      case $name in
        "walk-need dirty") echo "dirty $(date +%s%N)" >> "$work/notes.txt" ;;
        "walk-need resolve app.txt") echo "resolved" > "$work/app.txt" ;;
        "walk-need edit worktree walk-wt")
          # Where the walk's settings put new worktrees (../{repo}-trees/{branch}).
          wt=$W/work-trees/walk-wt
          echo "changed in the worktree" >> "$wt/app.txt"
          echo "new" > "$wt/new.txt"
          echo "scratch" > "$wt/scratch.txt"
          ;;
        "walk-need mark walk-marked")
          # An edit git status does not show: the file marked assume-unchanged.
          wt=$W/work-trees/walk-marked
          echo "a local tweak" >> "$wt/app.txt"
          git -C "$wt" update-index --assume-unchanged app.txt
          ;;
        "walk-need nest walk-marked")
          # Clean again, then a repository two levels inside an ignored folder.
          wt=$W/work-trees/walk-marked
          git -C "$wt" update-index --no-assume-unchanged app.txt
          git -C "$wt" checkout -q -- app.txt
          printf 'tmp/\n' >> "$(git -C "$wt" rev-parse --path-format=absolute --git-common-dir)/info/exclude"
          mkdir -p "$wt/tmp/refs"
          git init -q "$wt/tmp/refs/nested"
          echo mine > "$wt/tmp/refs/nested/mine.txt"
          git -C "$wt/tmp/refs/nested" add mine.txt
          git -C "$wt/tmp/refs/nested" -c user.name=Ada -c user.email=ada@example.invalid commit -q -m "nested work"
          ;;
        "walk-need fill walk-big")
          # An ignored node_modules of 300,000 entries (600 folders of 499 files), then a
          # branch to say it is done (the page sees refs change).
          wt=$W/work-trees/walk-big
          printf 'node_modules/\n' >> "$(git -C "$wt" rev-parse --path-format=absolute --git-common-dir)/info/exclude"
          python3 -I -c 'import os, sys
root = sys.argv[1]
for d in range(600):
    p = os.path.join(root, "p%d" % d)
    os.makedirs(p)
    for f in range(499):
        open(os.path.join(p, "f%d" % f), "w").close()' "$wt/node_modules"
          git -C "$wt" branch walk-big-filled
          ;;
        "walk-need list pr 21")
          printf '[{"number":21,"title":"Walk: a change in a worktree","url":"https://github.com/octo/cat/pull/21","isDraft":false,"headRefName":"walk/wt","baseRefName":"main","headRepositoryOwner":{"id":"X","login":"octo"},"isCrossRepository":false}]\n' \
            > "$W/gh/replies/pr-list.out"
          ;;
        "walk-need gg other")
          # A second launch hands the folder to the running app and exits: the real
          # launcher path, `open -n -b com.shakirul.gg.perf` (Task 13 review minor 1: only
          # while the first is still there, so this launch never becomes the app).
          if kill -0 "$pid" 2> /dev/null; then
            (cd "$W" && env -i HOME="$HOME" PATH=/usr/bin:/bin GIT_CONFIG_GLOBAL="$GIT_CONFIG_GLOBAL" \
              GIT_CONFIG_NOSYSTEM=1 "$W/gg" other) || echo "gg other failed" >&2
          else
            echo "the app is gone: gg other not run" >&2
          fi
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
echo "launches recorded (never started):"
[ -f "$W/launches.jsonl" ] && cat "$W/launches.jsonl"
if [ -f "$W/blocked.log" ]; then
  echo "BLOCKED (must not run):"
  cat "$W/blocked.log"
fi
cleanup
sleep 1
pgrep -f "^$app" > /dev/null && echo "warning: the app is still running" >&2
echo "repo: $W"
