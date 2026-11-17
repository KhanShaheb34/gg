#!/usr/bin/env bash
# Interleaved launches of plan 2's and plan 3's perf builds on r20k: spawn -> first rows and
# -> fully loaded, wall clock. Only pids started here are killed.
set -uo pipefail
out=$1; runs=${2:-8}
mkdir -p "$out"
: > "$out/gitconfig-empty"
P2=~/Dev/experiments/git-graph/target/p2/release/bundle/macos/GitGraph.app
P3=~/Dev/experiments/git-graph/target/release/bundle/macos/gg.app
repo=$HOME/.cache/gitgraph-perf/r20k
now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f\n", time * 1000'; }
mark() { awk -F'\t' -v n="$2" '$1 == n { print $3; exit }' "$1"; }
printf 'run\tbuild\tload\tspawn->first-rows\tspawn->fully-loaded\tnav->first-rows\n' | tee "$out/table.tsv"
for i in $(seq 1 "$runs"); do
  for b in p2 p3; do
    if [ $b = p2 ]; then bundle=$P2; exe=$bundle/Contents/MacOS/$(/usr/libexec/PlistBuddy -c "Print :CFBundleExecutable" "$bundle/Contents/Info.plist"); else bundle=$P3; exe=$bundle/Contents/MacOS/gg-desktop; fi
    log=$out/$b-$i.log
    load=$(uptime | sed 's/.*load averages*: *//' | cut -d' ' -f1)
    spawn=$(now_ms)
    open -n -a "$bundle" --env GG_OPEN="$repo" --env GG_PERF_LOG="$log" --env GG_CONFIG_DIR="$out/config-$b-$i" \
      --env SHELL="$SHELL" --env GIT_CONFIG_GLOBAL="$out/gitconfig-empty" --env GIT_CONFIG_NOSYSTEM=1 \
      --env GG_PERF_LAUNCHES="$out/launches.jsonl"
    pid=""
    for _ in $(seq 1 100); do pid=$(pgrep -n -f "^$exe") && break; sleep 0.05; done
    for _ in $(seq 1 600); do grep -q "^fully-loaded	" "$log" 2>/dev/null && grep -q "^first-rows	" "$log" && break; sleep 0.05; done
    fr=$(mark "$log" first-rows); fl=$(mark "$log" fully-loaded)
    nav=$(awk -F'\t' '$1 == "first-rows" { print $2; exit }' "$log")
    { printf 'spawn\t-1\t%s\n' "$spawn"; } >> "$log"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$i" "$b" "$load" "$(( ${fr:-0} - spawn ))" "$(( ${fl:-0} - spawn ))" "${nav%.*}" | tee -a "$out/table.tsv"
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
    for _ in $(seq 1 40); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
    sleep 2
  done
done
for b in p2 p3; do
  awk -F'\t' -v b=$b 'NR > 1 && $2 == b && $1 > 1 { print $4 }' "$out/table.tsv" | sort -n |
    awk -v b=$b '{ v[NR] = $1 } END { m = NR % 2 ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2; printf "%s spawn->first-rows median %s (min %s, max %s, n %d)\n", b, m, v[1], v[NR], NR }'
  awk -F'\t' -v b=$b 'NR > 1 && $2 == b && $1 > 1 { print $6 }' "$out/table.tsv" | sort -n |
    awk -v b=$b '{ v[NR] = $1 } END { m = NR % 2 ? v[(NR + 1) / 2] : (v[NR / 2] + v[NR / 2 + 1]) / 2; printf "%s nav->first-rows median %s (min %s, max %s)\n", b, m, v[1], v[NR] }'
done
