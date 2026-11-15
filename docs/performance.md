# Performance

gg is meant to be light enough to leave open all day. This page says what was measured, how,
on what, and where the app misses its own targets. Nothing is rounded in the app's favour.

## Summary

On a synthetic repository of 20,000 commits and 200 branches, release build, WKWebView:

| What | Target | Measured | Met? |
|---|---|---|---|
| Launch to the first rows of the graph | under 1 s | 0.74 s median (0.64–0.80) | yes |
| Whole history loaded and scrollable | under 2 s | 0.77 s median (0.62–0.84) | yes |
| Scrolling | 60 fps, no blank rows | p95 frame 19 ms against 17–18 ms when idle; 0 of 361 frames over 25 ms; no row blank 300 ms after stopping | yes (see below) |
| Feedback after a click | under 100 ms | p95 6 ms (worst launch 13 ms) | yes |
| Refresh after a change on disk | under 1 s, no scroll jump | 0.57–0.81 s, scroll position kept | yes |
| Idle CPU, window open | effectively zero | 0.00 % (app and its WebKit processes) | yes |
| Memory, one 20,000-commit repository open | under 150 MB | about 160 MB idle; about 250 MB after scrolling the whole history | **no** |
| App bundle | under 20 MB | 6.6 MB | yes |

## Machine and conditions

- An Apple M1 Pro laptop (8 cores, 16 GB), built-in Retina display, macOS 27.2, git 2.54.
- Measured in November 2026 on release builds, in the app's real window (WKWebView) at
  1280×800, unless a row says Chromium.
- The machine was in normal use while measuring, with a one-minute load average between about
  3 and 25. Launch times move by a few hundred milliseconds from run to run under such load;
  medians are more trustworthy than maxima, and a quiet machine would likely measure lower.
- The first launch of each series (the first launch of a freshly built binary, about 1.8 s)
  is listed separately and left out of the medians. A launch whose window was covered gets no
  animation frames from WebKit and cannot be measured; such launches were left out.
- Every launch used its own empty config folder, so no saved tabs or settings were involved.

## Test repositories

All are made by `scripts/gen-synthetic-repo.sh`, which is deterministic (the same arguments
always give the same commit hashes):

- **r20k**: 20,000 commits, 200 branches, about 500 merges, 27 tags, 102 remote-tracking
  branches, 8 authors.
- **r100k**: 100,000 commits, 400 branches, about 2,500 merges, 133 tags, 202
  remote-tracking branches.
- **r20k-tree**: r20k plus one commit adding 50,000 files, 3,000 untracked files and 10
  changed ones, so that `git status` has real work to do.

None of them has a commit-graph file (see "Known misses").

## How each number is taken

- **Launch.** `scripts/perf.sh` launches the app through LaunchServices (`open`), as a
  click in the Dock does, and takes the wall clock just before. The page marks the animation
  frame in which the first real rows are drawn (`first-rows`) and the moment a snapshot with
  the whole history is shown (`fully-loaded`); the figures are those marks minus the launch
  time.
- **Idle CPU.** 20 s after `fully-loaded`, with the window open and untouched, the cumulative
  CPU time of the app and its three WebKit processes is read twice, 10 s apart.
- **Memory.** The physical footprint (`footprint`) of the app plus its WebKit processes
  (WebContent, GPU, Networking), summed; the same measure Activity Monitor's "Memory" column
  uses. The WebKit processes are matched to the app by their responsible process.
- **Scrolling.** A probe built into measurement builds scrolls the graph from top to bottom
  in 200 equal jumps over 6 s (like dragging the scrollbar) and records every animation
  frame's duration, the longest time a visible row stayed unfilled, and the unfilled rows
  300 ms after stopping. A control launch records frames for the same 6 s without scrolling.
  WKWebView caps frames at 60 Hz and reports times in whole milliseconds, so a perfect
  stream reads 16–17 ms; the control's 17–18 ms is the machine's own jitter.
- **Click feedback.** The probe clicks 20 visible rows; the time runs from just before the
  click to the first frame in which the row is selected.
- **Actions.** On a scratch clone of r20k with a local `origin`: right-click a row, pick
  "Create branch here…", type a name, press Enter, ten times per launch; each step is timed
  to the frame that paints its result.
- **Refresh.** A commit is made, or a branch moved, with git from outside the app; the time
  runs from git's exit to the frame that shows the change.
- **Bundle.** `du -sh` of the `.app` built by `pnpm tauri build --bundles app`.

The measurement hooks exist only in a build with the `perf` Cargo feature; a normal build
has none of them.

## Results in detail

### r20k

| Measure | Median (range), n |
|---|---|
| Launch → first rows | 742 ms (635–802), 5 |
| Launch → fully loaded | 766 ms (617–839), 5 |
| Idle CPU | 0.00 % (0.00–0.00), 5 |
| Idle memory | 157.7 MB (155.5–164.4), 5: app 51.5, WebContent 82.0, GPU 17.4, Networking 5.8 |
| Memory right after a full jump scroll | 326.1 MB (323.0–331.2), 4 |
| Memory 30 s after a full jump scroll | 248.6 MB (243.4–281.7), 4 |
| Jump scroll frame p95 / p99 | 19 / 20 ms |
| Frames over 25 ms during the scroll | 0 of 361 |
| Longest a visible row stayed unfilled during the scroll | 135 ms (134–148) |
| Unfilled rows 300 ms after stopping | 0 |
| Click feedback p95 | 6 ms (3–13) |
| Page errors (script errors, rejections, CSP violations) | 0 |

During a jump of more than a screen per frame, rows are filled at most every 120 ms, so a
fast scrollbar drag shows briefly unfilled rows; a trackpad scroll fills every frame.

### Actions (r20k clone)

| Step | p50 | worst |
|---|---|---|
| Right-click → menu painted | 4 ms | 15 ms |
| Menu item → dialog painted | 10–11 ms | 24 ms |
| Enter → dialog shows it is working | 15–16 ms | 20 ms |
| Enter → new branch label painted in the graph (git, refresh and redraw) | 199–215 ms | 234 ms |

The toolbar's busy indicator appears only for operations that run longer than 150 ms; the
dialog's own state is the immediate feedback.

### Refresh after a change on disk

| Repository | Change | Times |
|---|---|---|
| r20k | New commit | 567–682 ms (median 584), n = 8 |
| r20k | Branch moved | 571–583 ms (median 577), n = 8 |
| r20k-tree | New commit | 665–813 ms (median 677), n = 8 |
| r20k-tree | File added in the working tree, window away and back | 313–394 ms (median 345), n = 8 |

In Chromium against the dev server, with the graph scrolled to row 10,000, the top visible
row stayed at the same offset (0 px moved) through every refresh.

### r20k-tree (large working tree)

First rows 710 ms (681–750), idle CPU 0.00 % (0.00–0.20), idle memory 156.0 MB
(149.8–157.4), n = 5. `git status` there takes about 0.1 s warm on this machine's SSD; a cold
cache or a slower disk was not measured. The rows do not wait for `git status`: the
uncommitted row arrives when it is known.

Each time the window loses and regains focus the shown repository is re-read, which costs
about 0.1 s of CPU in all. One earlier launch on this repository read 23.8 % idle CPU for no
reason that could be found; it did not happen again in 12 later launches.

### r100k

| Measure | Value |
|---|---|
| Launch → first rows | 0.91–1.01 s in most launches, up to 2.05 s |
| Launch → fully loaded | 1.2–1.9 s |
| Idle CPU | 0.00 % |
| Idle memory | 213.4 MB (the app process 102 MB, for 100,000 commits) |

### The core alone

`crates/gg-core/tests/perf.rs`, release build, median of three warm opens:

| Measure | Target | Measured |
|---|---|---|
| r20k: first 500 rows ready | under 300 ms | 173 ms |
| r20k: whole history loaded | under 1.5 s | 215 ms |
| r100k: whole history loaded | under 6 s | 759 ms |
| A page of 200 rows from memory | under 2 ms | 0.05 ms (0.3 ms more to serialise it) |
| Search "feature" on r100k | under 150 ms | 6.9 ms (7,430 hits) |

## Known misses

- **Memory, idle: about 158 MB against a target of 150 MB.** Most of it is fixed cost: the
  app process (Tauri, AppKit and the core holding 20,000 commits) is about 52 MB, and
  WebKit's three processes about 105 MB. A small repository (80 commits) idles at
  125–148 MB.
- **Memory after scrolling the whole history: about 250 MB 30 s later.** The page's own
  JavaScript heap does not grow with the rows scrolled past (measured in Chromium after a
  forced collection: 4.7 MB before, 6.2 MB after). What stays is memory WebKit's allocator
  keeps after drawing 20,000 different rows. It is cache, not live memory: in an earlier
  build, WebKit's memory-pressure notification brought the total back to about 153 MB
  within 5 s. Not filling rows at all during fast jumps saved about 50 MB, but left rows blank
  for up to 1.8 s during a scrollbar drag; filling them every 120 ms was chosen instead.
- **Launch on r100k: sometimes over 1 s.** `git log --date-order` on a repository without a
  commit-graph file walks the whole history before printing the first commit (0.5–0.8 s on
  r100k with git alone). With a commit-graph (`git commit-graph write --reachable`, which
  `git gc` and `git maintenance` also write), git's first output comes in about 0.1 s. gg
  does not write one for you: its scheduled background fetch runs no auto-maintenance and
  writes no commit-graph.
- **Scrolling frame times sit at the measurement's noise floor.** WKWebView reports whole
  milliseconds at 60 Hz, and the idle control's p95 is already 17–18 ms, so small
  improvements or regressions in per-frame work cannot be seen with this probe. Instruments
  cannot attach to WKWebView's web process, so WebKit's paint cost could not be attributed.

## Reproducing

macOS only for the app measurements. Keep the screen unlocked and the app's window uncovered
while `perf.sh` runs: a covered window gets no frames.

```sh
# Test repositories (deterministic; a few seconds each)
PERF=~/perf-repos
scripts/gen-synthetic-repo.sh "$PERF/r20k" 20000 200
scripts/gen-synthetic-repo.sh "$PERF/r100k" 100000 400
scripts/gen-synthetic-repo.sh --tree 50000 3000 "$PERF/r20k-tree" 20000 200

# The core alone
GG_PERF_R20K="$PERF/r20k" GG_PERF_R100K="$PERF/r100k" \
  cargo test -p gg-core --release --test perf -- --ignored --nocapture

# The app: builds it with the measurement hooks, then 6 launches (idle CPU and memory)
scripts/perf.sh "$PERF/r20k"
# Scrolling, interleaved with the still control and a trackpad-style scroll
scripts/perf.sh --no-build --modes "still 1 smooth" "$PERF/r20k"

# Chromium against a release dev server: scrolling, clicks, refresh, heap
GG_PERF=1 GG_PERF_R20K="$PERF/r20k" GG_PERF_R100K="$PERF/r100k" pnpm e2e e2e/perf --headed
```

The options of `scripts/perf.sh` are described at the top of the script. The probes that change a
repository (`--modes actions`, `walk`) run only on a throwaway repository made by
`scripts/walk-fixture.sh`; `scripts/walk.sh` drives a full walk through the menus, dialogs,
banner and log in the real window.
