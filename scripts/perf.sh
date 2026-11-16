#!/usr/bin/env bash
# Measures the release app (macOS) on one repository and prints a table.
#
#   scripts/perf.sh [--no-build] [--runs N] [--scroll | --still | --smooth | --mixed | --modes "M..."] <repo>
#
# Builds the bundle with its measurement hooks (`pnpm tauri build --bundles app --features
# perf`, skipped with --no-build), then launches it N times (default 6), one at a time, with
# `open -n -a gg.app --env ...`, the way a user's launch goes through LaunchServices.
# Every launch gets its own empty config folder (GG_CONFIG_DIR), so the user's own app state
# is never read or changed and no saved session adds tabs. The script refuses to run while
# any copy of the app (bundle id com.shakirul.gg) is running.
#
# - Launch: the wall-clock time (unix ms) is taken right before `open` runs ("spawn"). The app
#   appends `name<TAB>ms since navigation start<TAB>unix ms` to the perf log for each mark,
#   so spawn -> first-rows and spawn -> fully-loaded are wall-clock differences. They include
#   LaunchServices, process start, window and WebKit start-up, page load and the repository
#   load. (Executing the binary directly from a shell does not bring its window forward on
#   current macOS; a window that stays hidden behind others gets no animation frames, so
#   `first-rows`, which is marked from one, may never come. `open` activates it like a click
#   in the Dock does.) Keep the screen unlocked and the window uncovered.
# - Idle (default): after fully-loaded, waits 20 s with the window open, then measures the
#   CPU time used over the next 10 s by the app and its WebKit helpers (from their cumulative
#   `ps -o time` before and after, so it is an exact average), and their memory: physical
#   footprint (`footprint`) and resident size (`ps -o rss`), summed.
# - Scroll (--scroll): GG_PERF_SCROLL=1 makes the app scroll the graph top to bottom in 200
#   steps over 6 s after fully-loaded, then click 20 visible rows, and log frame-time and
#   click marks; memory is measured right after and again 30 s later, per process.
# - Still (--still): the control for --scroll: GG_PERF_SCROLL=still records frames for the
#   same 6 s without scrolling (then the same clicks).
# - Smooth (--smooth): GG_PERF_SCROLL=smooth scrolls for the same 6 s the way a trackpad does,
#   a few px every frame at a steady speed (most frames move the rows by less than a row).
# - Mixed (--mixed): N still and N scroll launches, alternating, so both see the same load.
#   --modes "still 1 smooth" interleaves any of: 0 (idle), 1 (scroll), still, smooth, actions, pages, worktrees
#   (pages: GG_PERF_SCROLL=pages loads every page of rows once without drawing them, then
#   runs the still control; it tells the memory of loading rows from that of drawing them;
#   actions: GG_PERF_SCROLL=actions times right-click -> menu, pick -> dialog, Enter -> dialog
#   closed and operation end -> graph updated, creating branches; walk: the whole walk, see
#   scripts/walk.sh. Both change the repository, so they run only on a throwaway repository
#   made by scripts/walk-fixture.sh, which marks it: on any other the script refuses;
#   worktrees: GG_PERF_SCROLL=worktrees times showing the side panel (Cmd+B -> its rows
#   painted) and "Make active" on another worktree (-> the switch shown, -> done), on a
#   repository with linked worktrees; it writes nothing to the repository).
#
# WebKit helpers: WKWebView runs its page in XPC services (com.apple.WebKit.WebContent,
# .Networking, .GPU) whose parent is launchd, not the app. They are matched by "responsible
# process", the attribution Activity Monitor uses, read with the (undocumented but
# unprivileged) libsystem call responsibility_get_pid_responsible_for_pid through Python's
# ctypes: every WebKit process whose responsible pid is the app's. The script prints them
# and warns if it does not find exactly one of each kind.
#
# The first launch is listed separately (right after a build it is the first launch of that
# binary: cold-ish); medians, minimums and maximums are over the other launches, per mode.
# The machine's load average is printed with every launch.
set -euo pipefail

build=1
runs=6
modes="0"
while [ $# -gt 0 ]; do
  case $1 in
    --no-build) build=0; shift ;;
    --runs) runs=$2; shift 2 ;;
    --scroll) modes="1"; shift ;;
    --still) modes="still"; shift ;;
    --smooth) modes="smooth"; shift ;;
    --mixed) modes="still 1"; shift ;;
    --modes) modes=$2; shift 2 ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) break ;;
  esac
done
if [ $# -ne 1 ] || [ ! -d "$1" ]; then
  echo "usage: $0 [--no-build] [--runs N] [--scroll | --still | --smooth | --mixed | --modes \"M...\"] <repo>" >&2
  exit 2
fi
repo=$(cd "$1" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
# The walk and the actions probe change the repository (branches, pushes, deletes on its
# origin): only on a throwaway one scripts/walk-fixture.sh made, never on the user's (R43).
for mode in $modes; do
  case $mode in
    walk | actions)
      if [ "$(cat "$repo/.git/gg-perf-fixture" 2>/dev/null)" != "$repo" ]; then
        echo "--modes $mode changes the repository: it runs only on a fixture made by" \
          "scripts/walk-fixture.sh, and $repo is not one" >&2
        exit 2
      fi
      ;;
  esac
done
bundle=$root/target/release/bundle/macos/gg.app
app=$bundle/Contents/MacOS/gg-desktop
bundle_id=com.shakirul.gg

if [ -n "$(lsappinfo find bundleid=$bundle_id)" ] || pgrep -f "/gg.app/Contents/MacOS/gg-desktop" > /dev/null; then
  echo "gg ($bundle_id) is running; quit every copy of it first" >&2
  exit 1
fi
if [ "$build" = 1 ]; then
  (cd "$root" && CARGO_INCREMENTAL=0 pnpm tauri build --bundles app --features perf >&2)
fi
[ -x "$app" ] || { echo "no app at $app (build it first)" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/gg-perf.XXXXXX")
pid=""
cleanup() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f\n", time * 1000'; }
load() { uptime | sed 's/.*load averages*: *//'; }
# WebKit helper processes the app with pid $1 is responsible for.
helpers_of() {
  ps -axo pid=,comm= | awk '$2 ~ /com\.apple\.WebKit\.(WebContent|Networking|GPU)$/ { print $1 }' |
    python3 -I -c '
import ctypes, sys
f = ctypes.CDLL(None).responsibility_get_pid_responsible_for_pid
f.restype, f.argtypes = ctypes.c_int, [ctypes.c_int]
app = int(sys.argv[1])
print(" ".join(p for p in sys.stdin.read().split() if f(int(p)) == app))' "$1"
}
# Value of perf mark $2 in log $1: column 2 (ms since navigation start, or the value) or,
# with $3 = 3, column 3 (unix ms).
mark() { awk -F'\t' -v n="$2" -v c="${3:-2}" '$1 == n { print $c; exit }' "$1"; }
# Waits up to $3 seconds for mark $2 in log $1.
wait_mark() {
  local deadline=$(( $(date +%s) + $3 ))
  until [ -f "$1" ] && grep -q "^$2	" "$1"; do
    if [ "$(date +%s)" -ge "$deadline" ]; then return 1; fi
    sleep 0.05
  done
}
# Cumulative CPU seconds of the given pids (sum).
cpu_secs() {
  ps -o time= -p "$(echo "$@" | tr ' ' ',')" | awk '{
    n = split($1, p, ":"); s = 0
    for (i = 1; i <= n; i++) s = s * 60 + p[i]
    t += s
  } END { printf "%.2f\n", t }'
}
# Physical footprint (MB) of one pid.
footprint_mb() {
  footprint -f bytes --noCategories -p "$1" 2>/dev/null |
    awk '/phys_footprint:/ { printf "%.1f\n", $2 / 1048576; exit }'
}
rss_mb() { ps -o rss= -p "$1" | awk '{ printf "%.1f\n", $1 / 1024 }'; }
name_of() { ps -o comm= -p "$1" | sed 's/.*\.WebKit\./WebKit./; s/.*\/gg-desktop$/gg-desktop/'; }
# Footprint of the app and its helpers: prints "total<TAB>name=MB name=MB ...".
footprints() {
  # Called as `IFS=<tab> read ... < <(footprints ...)`, which hands it that IFS.
  local IFS=$' \t\n' total=0 detail="" p f
  for p in $1 $(helpers_of "$1"); do
    f=$(footprint_mb "$p")
    total=$(awk -v a="$total" -v b="${f:-0}" 'BEGIN { print a + b }')
    detail="$detail $(name_of "$p")=${f}"
  done
  printf '%s\t%s\n' "$total" "$detail"
}
mode_name() { case $1 in 0) echo idle ;; 1) echo scroll ;; *) echo "$1" ;; esac; }
# Quits the launch under measurement and lets it and its WebKit helpers exit.
stop_app() {
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 40); do
    kill -0 "$pid" 2>/dev/null || [ -n "$(helpers_of "$pid")" ] || break
    sleep 0.25
  done
  pid=""
  sleep 2
}

echo "repo: $repo ($(git -C "$repo" rev-list --all --count) commits)"
echo "machine: $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores," \
  "$(( $(sysctl -n hw.memsize) / 1073741824 )) GB, macOS $(sw_vers -productVersion)"
echo "bundle: $(du -sh "$bundle" | cut -f1) ($bundle)"
echo

# One line per value: launch, mode, key, value.
results=$work/results.tsv
: > "$results"
printf 'run\tmode\tload (1/5/15 min)\tspawn->first-rows\tspawn->fully-loaded\tnav->first-rows\tnav->fully-loaded\tidle CPU %%\tfootprint MB\tRSS MB\n'
run=0
for _ in $(seq 1 "$runs"); do
  for mode in $modes; do
    run=$((run + 1))
    name=$(mode_name "$mode")
    log=$work/perf-$run.log
    run_load=$(load)
    spawn=$(now_ms)
    open -n -a "$bundle" --env GG_OPEN="$repo" --env GG_PERF_LOG="$log" \
      --env GG_PERF_SCROLL="$mode" --env GG_CONFIG_DIR="$work/config-$run"
    pid=$(pgrep -n -f "^$app" || true)
    [ -n "$pid" ] || { echo "run $run: the app did not start" >&2; exit 1; }
    missing=""
    for m in first-rows fully-loaded; do
      wait_mark "$log" $m 60 || { missing=$m; break; }
    done
    if [ -n "$missing" ]; then
      # A covered window gets no animation frames; this launch is left out, not the series.
      echo "run $run: no $missing mark within 60 s (screen locked or window covered?); skipped" >&2
      stop_app
      continue
    fi
    first=$(( $(mark "$log" first-rows 3) - spawn ))
    loaded=$(( $(mark "$log" fully-loaded 3) - spawn ))
    nav_first=$(mark "$log" first-rows)
    nav_loaded=$(mark "$log" fully-loaded)

    helpers=$(helpers_of "$pid")
    cpu="-"
    cpu_detail=""
    if [ "$mode" = 0 ]; then
      sleep 20
      c0=$(cpu_secs $pid $helpers)
      for p in $pid $helpers; do echo "$p $(cpu_secs $p)"; done > "$work/cpu0"
      t0=$(now_ms)
      sleep 10
      c1=$(cpu_secs $pid $helpers)
      t1=$(now_ms)
      cpu=$(awk -v a="$c0" -v b="$c1" -v t="$(( t1 - t0 ))" 'BEGIN { printf "%.2f", (b - a) * 100000 / t }')
      cpu_detail=$(while read -r p c; do
        awk -v n="$(name_of "$p")" -v a="$c" -v b="$(cpu_secs "$p")" -v t="$(( t1 - t0 ))" \
          'BEGIN { printf " %s=%.2f%%", n, (b - a) * 100000 / t }'
      done < "$work/cpu0")
    elif ! wait_mark "$log" scroll-done 90; then
      echo "run $run: no scroll-done mark within 90 s (window covered?); skipped" >&2
      stop_app
      continue
    fi
    helpers=$(helpers_of "$pid")
    rss=0; kinds=""
    for p in $pid $helpers; do
      rss=$(awk -v a="$rss" -v b="$(rss_mb "$p")" 'BEGIN { print a + b }')
      kinds="$kinds $(name_of "$p")"
    done
    IFS=$'\t' read -r foot detail < <(footprints "$pid")
    printf '%s\t%s\t%s\t%s ms\t%s ms\t%s ms\t%s ms\t%s\t%s\t%s\n' "$run" "$name" "$run_load" \
      "$first" "$loaded" "${nav_first%.*}" "${nav_loaded%.*}" "$cpu" "$foot" "$rss"
    echo "    footprint MB:$detail"
    [ -z "$cpu_detail" ] || echo "    idle CPU:$cpu_detail"
    for k in WebKit.WebContent WebKit.Networking WebKit.GPU; do
      c=$(echo "$kinds" | tr ' ' '\n' | grep -c "^$k\$" || true)
      [ "$c" = 1 ] || echo "    warning: found $c $k processes for this launch"
    done
    {
      printf '%s\t%s\t%s\t%s\n' "$run" "$name" first "$first" "$run" "$name" loaded "$loaded" \
        "$run" "$name" cpu "$cpu" "$run" "$name" foot "$foot" "$run" "$name" rss "$rss"
      for d in $detail; do printf '%s\t%s\tfoot-%s\t%s\n' "$run" "$name" "${d%%=*}" "${d#*=}"; done
    } >> "$results"
    if [ "$mode" != 0 ]; then
      echo "    probe:" $(awk -F'\t' '$1 ~ /^(scroll|click|page|action|side|wt)-/ && $1 != "scroll-done" && $1 !~ /^page-error / { printf "%s=%s ", $1, $2 }' "$log")
      awk -F'\t' '$1 ~ /^page-error / { print "    " $1 }' "$log"
      awk -F'\t' -v r="$run" -v m="$name" '$1 ~ /^(scroll|click|page|action|side|wt)-/ && $1 != "scroll-done" && $1 !~ /^page-error / { print r "\t" m "\t" $1 "\t" $2 }' "$log" >> "$results"
      # WebKit drops graphics memory (tiles) some time after scrolling; measure again.
      sleep 30
      IFS=$'\t' read -r later detail < <(footprints "$pid")
      echo "    footprint 30 s later: $later MB:$detail"
      printf '%s\t%s\tfoot30\t%s\n' "$run" "$name" "$later" >> "$results"
      for d in $detail; do printf '%s\t%s\tfoot30-%s\t%s\n' "$run" "$name" "${d%%=*}" "${d#*=}"; done >> "$results"
    fi

    stop_app
  done
done

echo
echo "launches 2-$run, per mode: median / min / max"
for name in $(cut -f2 "$results" | sort -u); do
  echo "  $name:"
  awk -F'\t' -v m="$name" '$1 > 1 && $2 == m && $4 != "-" { print $3 "\t" $4 }' "$results" |
    sort -k1,1 -k2,2n | awk -F'\t' '
      function flush() {
        if (n == 0) return
        med = n % 2 ? v[(n + 1) / 2] : (v[n / 2] + v[n / 2 + 1]) / 2
        printf "    %-30s %10.2f %10.2f %10.2f   (%d)\n", key, med, v[1], v[n], n
      }
      $1 != key { flush(); key = $1; n = 0 }
      { v[++n] = $2 }
      END { flush() }'
done
