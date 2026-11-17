# Performance

gg is meant to be light enough to leave open all day. This page says what was measured, how,
on what, and where the app misses its own targets. Nothing is rounded in the app's favour.

## Summary

gg 0.3.0 (worktrees, commit panel, pull requests, settings) on a synthetic repository of
20,000 commits and 200 branches, release build, in the app's real window (WKWebView):

| What | Target | Measured | Met? |
|---|---|---|---|
| Launch to the first rows of the graph | under 1 s | 0.81 s median (0.65–0.86) on a busy machine; about 0.66 s on a quiet one | yes |
| Whole history loaded and scrollable | under 2 s | 0.84 s median (0.67–0.90) | yes |
| Scrolling | 60 fps, no blank rows | p95 frame 19 ms against 17–18 ms when idle; 0 of 361 frames over 25 ms; no row blank 300 ms after stopping | yes (see below) |
| Feedback after a click | under 100 ms | p95 3–10 ms | yes |
| Showing the worktree panel, switching worktree | feedback under 100 ms | panel painted in 22 ms (p50); the switch shown in the next frame (4–5 ms), done in about 140 ms | yes |
| Refresh after a change on disk | under 1 s, no scroll jump | 0.57–0.81 s, scroll position kept | yes |
| Idle CPU, window open | effectively zero | 0.00 % (app and its WebKit processes, at most 0.10 %) | yes |
| Memory, one 20,000-commit repository open | under 150 MB | about 155 MB idle; about 285 MB 30 s after scrolling the whole history | **no** |
| App bundle | under 20 MB | 7.9 MB | yes |

The refresh and core-only numbers were measured on the build before worktrees; nothing since
changed that path, and the core's own numbers were re-checked on 0.3.0 (see below).

## Machine and conditions

- An Apple M1 Pro laptop (8 cores, 16 GB), built-in Retina display, macOS 27.2, git 2.54.
- Measured in November 2026 on release builds, in the app's real window (WKWebView) at
  1280×800, unless a row says Chromium.
- The machine was in normal use while measuring, with a one-minute load average between about
  3 and 25. Launch times move by a hundred milliseconds or more from run to run under such
  load; medians are more trustworthy than maxima, and a quiet machine measures lower.
- The first launch of each series (the first launch of a freshly built binary, about 1.8 s)
  is listed separately and left out of the medians. A launch whose window was covered gets no
  animation frames from WebKit and cannot be measured; such launches were left out.
- Every launch used its own empty config folder, so no saved tabs or settings were involved.
- To compare two builds fairly, they were launched alternately in one series, so that both
  saw the same load.

## Test repositories

All are made by `scripts/gen-synthetic-repo.sh`, which is deterministic (the same arguments
always give the same commit hashes):

- **r20k**: 20,000 commits, 200 branches, about 500 merges, 27 tags, 102 remote-tracking
  branches, 8 authors.
- **r100k**: 100,000 commits, 400 branches, about 2,500 merges, 133 tags, 202
  remote-tracking branches.
- **r20k-tree**: r20k plus one commit adding 50,000 files, 3,000 untracked files and 10
  changed ones, so that `git status` has real work to do.

None of them has a commit-graph file (see "Known misses"). The worktree timings use local
clones of r20k and r20k-tree (`git clone --local`) with linked worktrees added.

## How each number is taken

- **Launch.** `scripts/perf.sh` launches the app through LaunchServices (`open`), as a
  click in the Dock does, and takes the wall clock just before. The page marks the animation
  frame in which the first real rows are drawn (`first-rows`) and the moment a snapshot with
  the whole history is shown (`fully-loaded`); the figures are those marks minus the launch
  time. The measurement build also marks the native start-up and the page's phases;
  `scripts/launch-phases.py` splits a series kept with `--logs` into them.
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
- **Worktrees.** Ten rounds per launch of ⌘B (panel hidden, then shown: from the key to the
  frame that paints its rows), then ten rounds of "Make active" on another worktree's row
  (from the click to the frame that shows the switch on the row, and to the frame where the
  core's new snapshot marks it active).
- **Refresh.** A commit is made, or a branch moved, with git from outside the app; the time
  runs from git's exit to the frame that shows the change.
- **Bundle.** `du -sh` of the `.app` built by `pnpm tauri build --bundles app`.

The measurement hooks exist only in a build with the `perf` Cargo feature, made under its own
identifier (`com.shakirul.gg.perf`) so it never shares storage or a single-instance lock with
an installed gg. A normal build has none of them.

## Results in detail

### r20k

Median (range), n launches.

| Measure | 0.3.0 |
|---|---|
| Launch → first rows | 808 ms (652–863), 10 |
| Launch → fully loaded | 840 ms (671–900), 6 |
| Idle CPU | 0.00 % (max 0.10), 6 |
| Idle memory | 154.8 MB (152.3–159.2), 6: app 50–54, WebContent 80–82, GPU 15–16, Networking 5–6 |
| Memory right after a full jump scroll | 333.2 MB (330.0–336.7), 4 |
| Memory 30 s after a full jump scroll | 285.7 MB (282.3–297.0), 4: WebContent 212–220 |
| Jump scroll frame p95 / p99 | 19 / 21–22 ms |
| Frames over 25 ms during the scroll | 0 of 361 (the first launch of each series had 1) |
| Longest a visible row stayed unfilled during the scroll | 133–148 ms |
| Unfilled rows 300 ms after stopping | 0 |
| Click feedback p95 | 3–10 ms |
| Page errors (script errors, rejections, CSP violations) | 0 |

During a jump of more than a screen per frame, rows are filled at most every 120 ms, so a
fast scrollbar drag shows briefly unfilled rows; a trackpad scroll fills every frame.

### Across builds

The same measures on the two earlier builds: the first, which had the graph only, and the
one before worktrees, which added the actions. The middle column is that build measured
again, interleaved with 0.3.0 under the same load.

| Measure | Graph only | Before worktrees | Before worktrees, side by side | 0.3.0 |
|---|---|---|---|---|
| Launch → first rows | 808 ms (755–1220) | 742 ms (635–802) | 750 ms (604–814) | 808 ms (652–863) |
| Idle memory | 154.3 MB | 157.7 MB | 148.8 MB (147.5–150.5) | 154.8 MB (152.3–159.2) |
| 30 s after a jump scroll | 239.5 MB | 248.6 MB | 264.1 MB (228.0–273.7) | 285.7 MB (282.3–297.0) |
| Jump p95 / p99 | 18 / 20 ms | 19 / 20 ms | 18–19 / 19–20 ms | 19 / 21–22 ms |
| Click feedback p95 | 3 ms | 6 ms | 7–12 ms | 3–10 ms |
| Bundle | 5.8 MB | 6.6 MB | 6.8 MB | 7.9 MB |

The bundle grew with the UI (JavaScript 348 KB against 237 KB, CSS 82 KB against 54 KB). The
launch difference is discussed under "Known misses".

### Actions (r20k clone)

| Step | p50 | worst |
|---|---|---|
| Right-click → menu painted | 4 ms | 15 ms |
| Menu item → dialog painted | 10–11 ms | 24 ms |
| Enter → dialog shows it is working | 15–16 ms | 20 ms |
| Enter → new branch label painted in the graph (git, refresh and redraw) | 199–215 ms | 234 ms |

The toolbar's busy indicator appears only for operations that run longer than 150 ms; the
dialog's own state is the immediate feedback. Measured on the build before worktrees.

### Worktree panel and switching

`scripts/perf.sh --modes worktrees`, 4 launches × 10 rounds, medians of the launches' values
(min–max):

| Repository | Panel shown, p50 / p95 | Switch shown, p50 / p95 | Switch done, p50 / p95 |
|---|---|---|---|
| r20k clone, 4 worktrees (8 changed files each) | 22 / 26.5 ms (24–28) | 4.5 / 6 ms (5–6) | 139 / 141 ms (139–156) |
| r20k-tree clone, 2 worktrees (50,000 files in the main one) | 20 / 26.5 ms (24–27) | 4 / 5 ms (5–6) | 140 / 321.5 ms (306–339) |

The switch is done in about 140 ms; switching to the worktree with 50,000 files, whose
`git status` has to read them all, takes about 320 ms. The switch shows in the next frame
either way.

### Refresh after a change on disk

| Repository | Change | Times |
|---|---|---|
| r20k | New commit | 567–682 ms (median 584), n = 8 |
| r20k | Branch moved | 571–583 ms (median 577), n = 8 |
| r20k-tree | New commit | 665–813 ms (median 677), n = 8 |
| r20k-tree | File added in the working tree, window away and back | 313–394 ms (median 345), n = 8 |

In Chromium against the dev server, with the graph scrolled to row 10,000, the top visible
row stayed at the same offset (0 px moved) through every refresh. Measured on the build
before worktrees.

### r20k-tree (large working tree)

On 0.3.0, with the working tree watched: idle CPU 0.00 % (max 0.10), idle memory 153.2 MB
(152.7–155.1), launch → first rows 686 ms (667–774), n = 4. `git status` there takes about
0.1 s warm on this machine's SSD; a cold cache or a slower disk was not measured. The rows do
not wait for `git status`: the uncommitted row arrives when it is known.

On the first build, one launch on this repository read 23.8 % idle CPU for no reason that
could be found; it has not happened again in any later launch.

### r100k

Measured on the first build; nothing since changed how history is loaded.

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
| r20k: first 500 rows ready | under 300 ms | 173–174 ms |
| r20k: whole history loaded | under 1.5 s | 215–219 ms |
| r100k: whole history loaded | under 6 s | 759 ms |
| A page of 200 rows from memory | under 2 ms | 0.05 ms (0.3 ms more to serialise it) |
| Search "feature" on r100k | under 150 ms | 6.9 ms (7,430 hits) |

### The whole app, used end to end

`scripts/walk.sh` drives the real window through its real controls, with the real core, git
and `gg` command, on a throwaway repository whose `origin` git sends to a local bare
repository, and with a recording stand-in for `gh`, so that no network host is contacted. git
runs without the user's global and system config, and editors, terminals and the browser are
recorded rather than started.

The walk for 0.3.0 has 12 steps: settings (theme, date format), the staging routine (check
out with fast-forward, merge, push from the notice), the side panel, a new worktree made
active, files changed on disk counted in the panel by themselves (seen 620–680 ms after the
change), committing some of them, pushing from the commit notice, a pull request (its marker
then on the worktree), deleting a clean worktree, deleting one with work in it (the red
confirmation lists the untracked file), and `gg` in a terminal into the running app.

| Run | Result |
|---|---|
| 1280×800, light and dark | 12 / 12, 0 page errors |
| 900×560, light and dark | 12 / 12, 0 page errors (one run first found a settings bug, fixed before release) |
| 1280×800, app started by `gg` | 12 / 12; the window painted 606 ms after navigation |
| The actions walk (23 steps: menus, dialogs, banner, log) | 23 / 23, 0 page errors |

The same walk runs in the browser build (`GG_WALK=1 pnpm e2e walk`, Chromium against
gg-devserver) at both sizes and in both themes.

## Known misses

- **Memory, idle: about 155 MB against a target of 150 MB.** Missed in every build so far
  (151–160 MB). Most of it is fixed cost: the app process (Tauri, AppKit and the core holding
  20,000 commits) is about 52 MB, and WebKit's three processes about 100 MB. A small
  repository (80 commits) idles at 125–148 MB.
- **Memory after scrolling the whole history: about 285 MB 30 s later**, up from about 250 MB
  before worktrees. The page's own JavaScript heap does not grow with the rows scrolled past
  (measured in Chromium after a forced collection: 4.7 MB before, 6.2 MB after). What stays is
  memory WebKit's allocator keeps after drawing 20,000 different rows. It is cache, not live
  memory: in an earlier build, WebKit's memory-pressure notification brought the total back
  to about 153 MB within 5 s. Not filling rows at all during fast jumps saved about 50 MB, but
  left rows blank for up to 1.8 s during a scrollbar drag; filling them every 120 ms was
  chosen instead. Against the build before worktrees, measured side by side, 0.3.0 uses 6 MB
  more idle and 22 MB more after scrolling, all of it in WebKit's WebContent process (the app
  process is the same). The larger stylesheet and the side panel, now shown by default at
  1280 px, are the likely contributors; neither was measured on its own.
- **Launch: slower than the build before worktrees in one series, not in another.** One
  interleaved series under heavy load showed 0.3.0's time from page navigation to the first
  rows at 431 ms against 158 ms. Profiling the launch phases found:
  - WebKit runs the page's script about 530 ms after launch (526–548 ms in every series),
    whenever navigation started; the HTML arrives within 12 ms, and serving and running the
    script take under 10 ms. So "navigation → first rows" mostly measures when navigation
    happened to start relative to that fixed wait, which is inside WebKit's processes, not in
    gg.
  - The size of the script is not the cause: loading dialogs on first use halved the start-up
    script (349 KB to 167 KB) and changed nothing (652–694 ms against 655–670 ms), so it was
    reverted.
  - A slow login shell is a cause. gg reads git's `PATH` from your login shell before it can
    answer the app's state; that takes about 190–250 ms here and normally ends before the page
    asks, but on a cold first launch it took about 600 ms and the state waited 200–270 ms, and
    under heavy load it is slower still. The shell now starts about 60 ms after the process.
  - Measured again side by side on a quieter day, the two builds were 705 ms (661–718) and
    721 ms (698–871) from launch to the first rows, inside the noise. Why the loaded series
    showed a 273 ms gap is not fully explained: its logs held only the launch and the two
    marks, not the phases.
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

## Not measured

- What WebKit does before it runs the page's script: nothing on WebKit's side can be timed
  from the app.
- Memory with the side panel hidden, to tell its share of the WebContent increase.
- The cost of the dark theme: the walk ran in both themes, but nothing was timed in dark.
- Windows and Linux: the numbers above are macOS only.

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

# The app: builds it with the measurement hooks, then 6 launches (idle CPU and memory),
# keeping each launch's log in perf-logs/ (git-ignored) for scripts/launch-phases.py
scripts/perf.sh --logs perf-logs/r20k-idle "$PERF/r20k"
scripts/launch-phases.py perf-logs/r20k-idle
# Scrolling, interleaved with the still control and a trackpad-style scroll
scripts/perf.sh --no-build --modes "still 1 smooth" "$PERF/r20k"
# The worktree panel and switching, on a repository with linked worktrees
scripts/perf.sh --no-build --modes worktrees path/to/clone-with-worktrees

# Chromium against a release dev server: scrolling, clicks, refresh, heap
GG_PERF=1 GG_PERF_R20K="$PERF/r20k" GG_PERF_R100K="$PERF/r100k" pnpm e2e e2e/perf --headed
```

The options of `scripts/perf.sh` are described at the top of the script. The probes that
change a repository (`--modes actions`, `walk`) run only on a throwaway repository made by
`scripts/walk-fixture.sh`; `scripts/walk.sh` drives a whole walk through the real window
(`--mode walk` for the actions, `--mode walk3` or `walk3-dark` for the worktree walk).
