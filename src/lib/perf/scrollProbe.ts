// Scroll measurement for performance runs (plan 1, task 10). Dormant unless a run asks for
// it: `window.__GG_PROBE__` (an object) turns on the graph's probe, and
// `window.__GG_PERF_SCROLL__ = true` makes the graph run `runScrollProbe` once its
// repository has fully loaded. The Playwright perf spec sets both in Chromium; the Tauri app
// sets both when started with GG_PERF_SCROLL=1, so the same scroll is measured in WKWebView.
// `__GG_PERF_SCROLL__ = "still"` (GG_PERF_SCROLL=still) is the control: the same 6 s of
// frame recording without scrolling, which shows the frame-time jitter of the engine and
// machine when the app does nothing. `__GG_PERF_SCROLL__ = "smooth"` (GG_PERF_SCROLL=smooth)
// scrolls for the same time the way a trackpad does: a few px every frame, at a steady speed,
// so most frames move the rows by less than one row.

/** Filled in by the graph view while the probe is on (see GraphView.svelte). */
export type Probe = {
  /** rAF deltas between consecutive frames that followed a scroll event, ms. */
  frames: number[];
  /** Longest time a visible row stayed a skeleton, ms. */
  skeletonMaxMs: number;
  /** Frames in which some visible row had been a skeleton for more than 150 ms. */
  slowSkeletonFrames: number;
  /** Set by `runScrollProbe` when it finishes. */
  summary?: ScrollSummary;
};

export type ScrollSummary = {
  /** Every rAF delta while scrolling, ms: the frame times. */
  frames: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  /** Deltas over 17 ms (later than 60 Hz by any amount). */
  over17: number;
  /** Deltas over 25 ms (half a frame or more late at 60 Hz: a dropped frame). */
  over25: number;
  skeletonMaxMs: number;
  slowSkeletonFrames: number;
  /** Visible rows still skeletons 300 ms after the last scroll step. */
  skeletonsAfterStop: number;
};

/** Nearest-rank percentile of `values` (0 for none); `p` in 0..100. */
export function percentile(values: readonly number[], p: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length, Math.max(1, rank)) - 1];
}

export function summarize(deltas: readonly number[], probe: Probe, skeletonsAfterStop: number): ScrollSummary {
  return {
    frames: deltas.length,
    p50: percentile(deltas, 50),
    p95: percentile(deltas, 95),
    p99: percentile(deltas, 99),
    max: deltas.length ? Math.max(...deltas) : 0,
    over17: deltas.filter((d) => d > 17).length,
    over25: deltas.filter((d) => d > 25).length,
    skeletonMaxMs: probe.skeletonMaxMs,
    slowSkeletonFrames: probe.slowSkeletonFrames,
    skeletonsAfterStop,
  };
}

/** Scroll target of step `k` of `steps` from the top to `maxTop`, px (whole pixels). */
export function stepTop(k: number, steps: number, maxTop: number): number {
  return Math.round((maxTop * k) / steps);
}

/** Scroll position `elapsedMs` into a smooth scroll at `pxPerSec`, px (whole pixels). */
export function smoothTop(elapsedMs: number, pxPerSec: number, maxTop: number): number {
  return Math.min(maxTop, Math.round((elapsedMs * pxPerSec) / 1000));
}

/**
 * How the probe scrolls: `jump` in `steps` equal steps from top to bottom (hundreds of rows
 * each), `smooth` by a few px every frame at `pxPerSec`, `still` not at all (the control).
 */
export type ScrollMode = "jump" | "smooth" | "still";

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Scrolls `scroller` for `durationMs` as `mode` says (see ScrollMode), recording every
 * requestAnimationFrame delta meanwhile; then waits `settleMs` and counts the visible rows
 * that are still skeletons. Stores and returns the summary.
 */
export async function runScrollProbe(
  scroller: HTMLElement,
  probe: Probe,
  {
    mode = "jump" as ScrollMode,
    steps = 200,
    durationMs = 6000,
    settleMs = 300,
    // About one row every second frame at 60 Hz: a brisk two-finger scroll.
    pxPerSec = 800,
  } = {},
): Promise<ScrollSummary> {
  scroller.scrollTop = 0;
  await sleep(500);
  probe.frames.length = 0;
  probe.skeletonMaxMs = 0;
  probe.slowSkeletonFrames = 0;

  const deltas: number[] = [];
  const maxTop = scroller.scrollHeight - scroller.clientHeight;
  const started = performance.now();
  let last = 0;
  let running = true;
  const tick = (now: number) => {
    if (last > 0) deltas.push(now - last);
    last = now;
    // Positioned by the time since the start, so a late frame scrolls further, as a
    // trackpad's momentum does.
    if (mode === "smooth" && running) scroller.scrollTop = smoothTop(now - started, pxPerSec, maxTop);
    if (running) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  for (let k = 1; k <= steps; k++) {
    // Steps are timed from the start, so a late step does not push the rest back.
    const wait = started + (k * durationMs) / steps - performance.now();
    if (wait > 0) await sleep(wait);
    if (mode === "jump") scroller.scrollTop = stepTop(k, steps, maxTop);
  }
  running = false;

  await sleep(settleMs);
  // Visible means between the sticky header and the bottom of the scroller.
  const top = scroller.querySelector("[data-header]")?.getBoundingClientRect().bottom ?? scroller.getBoundingClientRect().top;
  const bottom = scroller.getBoundingClientRect().bottom;
  let skeletons = 0;
  for (const row of scroller.querySelectorAll<HTMLElement>('[role="row"][aria-busy="true"]:not([hidden])')) {
    const r = row.getBoundingClientRect();
    if (r.bottom > top && r.top < bottom) skeletons++;
  }
  probe.summary = summarize(deltas, probe, skeletons);
  return probe.summary;
}

/** Click feedback: from the click to the start of the frame that paints the selection. */
export type ClickSummary = { clicks: number; p50: number; p95: number; max: number };

const nextFrame = () => new Promise<number>((resolve) => requestAnimationFrame(() => resolve(performance.now())));

/**
 * Clicks `count` different visible rows of the graph in `scroller` (a `click` event on the
 * row's subject, as the graph handles selection on click), each time measuring from just
 * before the event to the first animation frame in which the row is selected, i.e. the
 * frame that paints it. Rows are clicked 100 ms apart.
 */
export async function runClickProbe(scroller: HTMLElement, count = 20): Promise<ClickSummary> {
  const latencies: number[] = [];
  const top = scroller.querySelector("[data-header]")?.getBoundingClientRect().bottom ?? 0;
  const bottom = scroller.getBoundingClientRect().bottom;
  for (let k = 0; k < count; k++) {
    const rows = [...scroller.querySelectorAll<HTMLElement>('[role="row"][aria-busy="false"]:not([hidden])')].filter(
      (row) => {
        const r = row.getBoundingClientRect();
        return r.top >= top && r.bottom <= bottom && row.getAttribute("aria-selected") !== "true";
      },
    );
    const row = rows[k % Math.max(1, rows.length)];
    const subject = row?.querySelector<HTMLElement>(".subject");
    if (!row || !subject) break;
    const index = row.dataset.index;
    const started = performance.now();
    subject.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    for (let f = 0; f < 60; f++) {
      const at = await nextFrame();
      const now = scroller.querySelector(`[role="row"][data-index="${index}"]`);
      if (now?.getAttribute("aria-selected") === "true") {
        latencies.push(at - started);
        break;
      }
    }
    await sleep(100);
  }
  return {
    clicks: latencies.length,
    p50: percentile(latencies, 50),
    p95: percentile(latencies, 95),
    max: latencies.length ? Math.max(...latencies) : 0,
  };
}
