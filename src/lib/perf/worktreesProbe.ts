// Performance runs only (GG_PERF_SCROLL=worktrees, loaded with import()): opening the side
// panel and switching worktrees in the real window, on a repository with linked worktrees.
// Each time is from just before the event to the first animation frame after the change is in
// the DOM (the frame that paints it), as in actionsProbe.ts:
//   - the side panel shown with ⌘B / Ctrl+B -> its worktree rows painted (side-open-*);
//   - "Make active" on another worktree's row -> the row shows the switch, its spinner or the
//     active mark (wt-feedback-*: the < 100 ms feedback), and -> the switch done: that row
//     marked active by the core's new snapshot, no operation running (wt-switch-*).
// Nothing is written to the repository: making a worktree active is the app's own state.

import type { RepoView } from "../state/repo.svelte";
import { percentile } from "./scrollProbe";

const nextFrame = () => new Promise<number>((resolve) => requestAnimationFrame(() => resolve(performance.now())));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function paintedWhen(ready: () => boolean, frames = 600): Promise<number | null> {
  for (let f = 0; f < frames; f++) {
    const at = await nextFrame();
    if (ready()) return at;
  }
  return null;
}

const list = () => document.querySelector<HTMLElement>('[data-testid="worktree-list"]');
const rows = () => [...(list()?.querySelectorAll<HTMLElement>('[role="row"]') ?? [])];

function toggleKey(): void {
  const mac = navigator.platform.toLowerCase().includes("mac");
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: mac, ctrlKey: !mac, bubbles: true, cancelable: true }));
}

function stats(prefix: string, values: number[]): [string, number][] {
  const sorted = [...values].sort((a, b) => a - b);
  return [
    [`${prefix}-count`, values.length],
    [`${prefix}-p50`, percentile(sorted, 50)],
    [`${prefix}-p95`, percentile(sorted, 95)],
    [`${prefix}-max`, sorted.at(-1) ?? 0],
  ];
}

/** Runs `count` rounds of each; returns the marks. */
export async function runWorktreesProbe(view: RepoView, count = 10): Promise<[string, number][]> {
  const open: number[] = [];
  const feedback: number[] = [];
  const switched: number[] = [];
  let missed = 0;

  // Rows read first (the worktree list comes after the graph).
  await paintedWhen(() => view.snapshot.worktrees.length > 1 && rows().length > 1, 1200);
  for (let i = 0; i < count; i++) {
    if (list()) {
      toggleKey();
      await paintedWhen(() => !list());
      await sleep(300);
    }
    const t0 = performance.now();
    toggleKey();
    const at = await paintedWhen(() => rows().length > 1);
    if (at === null) missed++;
    else open.push(at - t0);
    await sleep(300);
  }

  for (let i = 0; i < count; i++) {
    const target = rows().find((r) => r.dataset.active !== "true" && r.querySelector("button.activate"));
    if (!target) {
      missed++;
      break;
    }
    const path = target.dataset.path ?? "";
    const button = target.querySelector<HTMLButtonElement>("button.activate")!;
    const t0 = performance.now();
    button.click();
    const row = () => rows().find((r) => r.dataset.path === path);
    const fb = await paintedWhen(() => {
      const r = row();
      return !!r && (r.dataset.active === "true" || r.querySelector(".spin") !== null);
    });
    const done = await paintedWhen(() => row()?.dataset.active === "true" && view.snapshot.op === null, 1200);
    if (fb === null || done === null) missed++;
    else {
      feedback.push(fb - t0);
      switched.push(done - t0);
    }
    await sleep(500);
  }

  return [...stats("side-open", open), ...stats("wt-feedback", feedback), ...stats("wt-switch", switched), ["wt-missed", missed]];
}
