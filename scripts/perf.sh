#!/usr/bin/env bash
# Measures the release app (macOS) on one repository and prints a table.
#
#   scripts/perf.sh [--no-build] [--runs N] [--scroll | --still] <repo>
#
# Builds the bundle (`pnpm tauri build --bundles app`, skipped with --no-build), then launches
# it N times (default 6), one at a time, with `open -n -a GitGraph.app --env ...` (GG_OPEN=<repo>,
# GG_PERF_LOG, GG_PERF_SCROLL), the way a user's launch goes through LaunchServices:
#
# - Launch: the wall-clock time (unix ms) is taken right before `open` runs ("spawn"). The app
#   appends `name<TAB>ms since navigation start<TAB>unix ms` to the perf log for each mark,
#   so spawn -> first-rows and spawn -> fully-loaded are wall-clock differences. They include
#   LaunchServices, process start, window and WebKit start-up, page load and the repository
#   load. (Executing the binary directly from a shell does not bring its window forward on
#   current macOS; a window that stays hidden behind others gets no animation frames, so
#   `first-rows`, which is marked from one, may never come. `open` activates it like a click
#   in the Dock does.)
# - Idle (without --scroll): after fully-loaded, waits 20 s with the window open, then
#   measures the CPU time used over the next 10 s by the app and its WebKit helpers (from
#   their cumulative `ps -o time` before and after, so it is an exact average), and their
#   memory: physical footprint (`footprint`) and resident size (`ps -o rss`), summed.
# - Scroll (--scroll): GG_PERF_SCROLL=1 makes the app scroll the graph top to bottom in 200
#   steps over 6 s after fully-loaded and log frame-time marks; their values are printed,
#   then memory is measured as above (no idle wait), and the footprint again 30 s later.
# - Still (--still): the control for --scroll: GG_PERF_SCROLL=still records frame times for
#   the same 6 s without scrolling, showing the engine's and machine's own frame jitter.
#
# WebKit helpers: WKWebView runs its page in XPC services (com.apple.WebKit.WebContent,
# .Networking, .GPU) whose parent is launchd, not the app. They are matched by "responsible
# process", the attribution Activity Monitor uses, read with the (undocumented but
# unprivileged) libsystem call responsibility_get_pid_responsible_for_pid through Python's
# ctypes: every WebKit process whose responsible pid is the app's. The script prints them
# and warns if it does not find exactly one of each kind.
#
# The first launch is listed separately (right after a build it is the first launch of that
# binary: cold-ish); medians and minimums are over the other runs. The machine's load
# average is printed with every run.
#
# The app's config folder is moved aside for the measurement (so no saved session opens other
# tabs) and put back afterwards.
set -euo pipefail

build=1
runs=6
scroll=0
mode=0
while [ $# -gt 0 ]; do
  case $1 in
    --no-build) build=0; shift ;;
    --runs) runs=$2; shift 2 ;;
    --scroll) scroll=1; mode=1; shift ;;
    --still) scroll=1; mode=still; shift ;;
    -*) echo "unknown option $1" >&2; exit 2 ;;
    *) break ;;
  esac
done
if [ $# -ne 1 ] || [ ! -d "$1" ]; then
  echo "usage: $0 [--no-build] [--runs N] [--scroll | --still] <repo>" >&2
  exit 2
fi
repo=$(cd "$1" && pwd)
root=$(cd "$(dirname "$0")/.." && pwd)
bundle=$root/target/release/bundle/macos/GitGraph.app
app=$bundle/Contents/MacOS/gitgraph

if [ "$build" = 1 ]; then
  (cd "$root" && CARGO_INCREMENTAL=0 pnpm tauri build --bundles app >&2)
fi
[ -x "$app" ] || { echo "no app at $app (build it first)" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/gg-perf.XXXXXX")
config="$HOME/Library/Application Support/dev.gitgraph.desktop"
saved=""
if [ -e "$config" ]; then
  saved="$work/config-backup"
  mv "$config" "$saved"
fi
pid=""
cleanup() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
  rm -rf "$config"
  if [ -n "$saved" ]; then mv "$saved" "$config"; fi
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
name_of() { ps -o comm= -p "$1" | sed 's/.*\.WebKit\./WebKit./; s/.*\/gitgraph$/gitgraph/'; }
# Median and minimum of the numbers on stdin.
stats() { sort -n | awk '{ v[NR] = $1 } END {
  if (NR == 0) { print "-\t-"; exit }
  m = NR % 2 ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2
  printf "%.2f\t%.2f\n", m, v[1] }'; }

if pgrep -f "^$app" > /dev/null; then
  echo "GitGraph is already running from $bundle; quit it first" >&2
  exit 1
fi

echo "repo: $repo ($(git -C "$repo" rev-list --all --count) commits)"
echo "machine: $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores," \
  "$(( $(sysctl -n hw.memsize) / 1073741824 )) GB, macOS $(sw_vers -productVersion)"
echo "bundle: $(du -sh "$bundle" | cut -f1) ($bundle)"
echo

results=$work/results.tsv
: > "$results"
printf 'run\tload (1/5/15 min)\tspawn->first-rows\tspawn->fully-loaded\tnav->first-rows\tnav->fully-loaded\tidle CPU %%\tfootprint MB\tRSS MB\n'
for run in $(seq 1 "$runs"); do
  rm -rf "$config"
  log=$work/perf-$run.log
  run_load=$(load)
  spawn=$(now_ms)
  open -n -a "$bundle" --env GG_OPEN="$repo" --env GG_PERF_LOG="$log" --env GG_PERF_SCROLL="$mode"
  pid=$(pgrep -n -f "^$app" || true)
  [ -n "$pid" ] || { echo "run $run: the app did not start" >&2; exit 1; }
  for m in first-rows fully-loaded; do
    if ! wait_mark "$log" $m 60; then
      echo "run $run: no $m mark within 60 s" >&2
      exit 1
    fi
  done
  first=$(( $(mark "$log" first-rows 3) - spawn ))
  loaded=$(( $(mark "$log" fully-loaded 3) - spawn ))
  nav_first=$(mark "$log" first-rows)
  nav_loaded=$(mark "$log" fully-loaded)

  helpers=$(helpers_of "$pid")
  if [ "$scroll" = 1 ]; then
    wait_mark "$log" scroll-done 60 || { echo "run $run: no scroll-done mark" >&2; exit 1; }
    cpu="-"
  else
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
  fi
  helpers=$(helpers_of "$pid")
  foot=0; rss=0; detail=""; kinds=""
  for p in $pid $helpers; do
    f=$(footprint_mb "$p"); r=$(rss_mb "$p"); n=$(name_of "$p")
    foot=$(awk -v a="$foot" -v b="${f:-0}" 'BEGIN { print a + b }')
    rss=$(awk -v a="$rss" -v b="${r:-0}" 'BEGIN { print a + b }')
    detail="$detail $n[$p]=${f}MB"
    kinds="$kinds $n"
  done
  printf '%s\t%s\t%s ms\t%s ms\t%s ms\t%s ms\t%s\t%s\t%s\n' \
    "$run" "$run_load" "$first" "$loaded" "${nav_first%.*}" "${nav_loaded%.*}" "$cpu" "$foot" "$rss"
  echo "    processes:$detail"
  [ "$scroll" = 1 ] || echo "    idle CPU:$cpu_detail"
  for k in WebKit.WebContent WebKit.Networking WebKit.GPU; do
    c=$(echo "$kinds" | tr ' ' '\n' | grep -c "^$k\$" || true)
    [ "$c" = 1 ] || echo "    warning: found $c $k processes for this launch"
  done
  if [ "$scroll" = 1 ]; then
    echo "    scroll:" $(awk -F'\t' '$1 ~ /^scroll-/ && $1 != "scroll-done" { printf "%s=%s ", substr($1, 8), $2 }' "$log")
    awk -F'\t' '$1 ~ /^scroll-/ && $1 != "scroll-done" { print substr($1, 8) "\t" $2 }' "$log" |
      sed "s/^/$run\tscroll-/" >> "$results"
    # WebKit drops graphics memory (tiles) some time after scrolling; measure again.
    sleep 30
    later=0
    for p in $pid $(helpers_of "$pid"); do
      later=$(awk -v a="$later" -v b="$(footprint_mb "$p")" 'BEGIN { print a + b }')
    done
    echo "    footprint 30 s later: $later MB"
    printf '%s	foot30	%s\n' "$run" "$later" >> "$results"
  fi
  printf '%s\tfirst\t%s\n%s\tloaded\t%s\n%s\tcpu\t%s\n%s\tfoot\t%s\n%s\trss\t%s\n' \
    "$run" "$first" "$run" "$loaded" "$run" "$cpu" "$run" "$foot" "$run" "$rss" >> "$results"

  kill "$pid"
  # Let the app and its WebKit helpers exit before the next launch.
  for _ in $(seq 1 40); do
    kill -0 "$pid" 2>/dev/null || [ -n "$(helpers_of "$pid")" ] || break
    sleep 0.25
  done
  pid=""
  sleep 2
done

echo
if [ "$runs" -gt 1 ]; then
  echo "runs 2-$runs (warm): median / min"
  for key in first loaded cpu foot foot30 rss; do
    awk -F'\t' -v k="$key" '$1 > 1 && $2 == k && $3 != "-" { print $3 }' "$results" | stats |
      awk -F'\t' -v k="$key" '{ printf "  %-8s median %s  min %s\n", k, $1, $2 }'
  done
  if [ "$scroll" = 1 ]; then
    for key in p50 p95 p99 max over17 over25 skeleton-max slow-skeleton-frames skeletons-after-stop; do
      awk -F'\t' -v k="scroll-$key" '$1 > 1 && $2 == k { print $3 }' "$results" |
        sort -n | awk -v k="$key" '{ v[NR] = $1 } END {
          if (NR == 0) exit
          m = NR % 2 ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2
          printf "  %-22s median %.1f  min %.1f  max %.1f\n", k, m, v[1], v[NR] }'
    done
  fi
fi
